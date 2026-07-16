use async_trait::async_trait;

use super::{
    ContextCompactionPlan, ContextCompactionRequest, ContextCompactionTrigger, ContextCompactor,
};
use crate::{
    error::ContextCompactionError,
    provider::LlmProvider,
    types::{
        Content, ContentPart, GenerationConfig, Message, ProviderRequest, ReasoningEffort, Role,
    },
};

/// Maximum output budget used by the default full-conversation summarizer.
pub const DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS: u32 = 20_000;
/// Headroom retained after reserving the summarizer's output budget.
pub const DEFAULT_CONTEXT_COMPACTION_BUFFER_TOKENS: u64 = 13_000;
/// Maximum number of prompt-too-long truncation retries for the summary call.
pub const DEFAULT_CONTEXT_COMPACTION_MAX_RETRIES: usize = 3;

const SUMMARY_SYSTEM_PROMPT: &str =
    "You are a helpful AI assistant tasked with summarizing conversations.";
const COMPACT_BOUNDARY_MESSAGE: &str = "Conversation compacted";
const COMPACT_RETRY_MARKER: &str =
    "[earlier conversation truncated while recovering the compaction request]";

const BASE_COMPACT_PROMPT: &str = r#"CRITICAL: Respond with TEXT ONLY. Do not call tools.

Create a detailed summary of the conversation so far. Preserve the user's explicit requests, the work already performed, and every detail needed to continue without access to the original history.

Before the final summary, use an <analysis> block as private drafting space. Then provide the durable result in a <summary> block. The <analysis> block will be removed before the result is retained.

The <summary> must cover:
1. Primary request and intent.
2. Important technical concepts, constraints, and architectural decisions.
3. Files and code sections examined, created, or modified, including important signatures and snippets when necessary.
4. Errors encountered, their causes, and fixes.
5. Problems solved and unresolved investigations.
6. Every user-authored message that is not a tool result.
7. Pending tasks and explicit acceptance criteria.
8. The work in progress immediately before compaction.
9. The next step that directly continues the latest request.

Be chronological, precise, and technically complete. Pay special attention to corrections or preferences stated by the user.

REMINDER: output plain text only: one <analysis> block followed by one <summary> block. Tool calls will make the compaction fail."#;

/// Returns the default fixed-headroom automatic compaction threshold.
///
/// The effective context reserves up to 20k tokens for the summary response,
/// then keeps a further 13k-token buffer for the next model request.
pub fn default_context_compaction_threshold(
    max_context_tokens: u64,
    max_output_tokens: Option<u32>,
) -> u64 {
    let summary_reserve = u64::from(
        max_output_tokens
            .unwrap_or(DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS)
            .clamp(1, DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS),
    );
    max_context_tokens
        .saturating_sub(summary_reserve)
        .saturating_sub(DEFAULT_CONTEXT_COMPACTION_BUFFER_TOKENS)
}

/// Default full-conversation compaction strategy.
///
/// The implementation uses the Agent's selected model with tools disabled,
/// strips media and opaque provider replay state from the summary request,
/// and atomically replaces active history with a boundary plus a synthetic
/// user summary. It keeps no session state, so one instance can safely be
/// selected independently by many Agents.
#[derive(Clone, Debug)]
pub struct DefaultContextCompactor {
    max_summary_tokens: u32,
    auto_compact_buffer_tokens: u64,
    max_compact_retries: usize,
}

impl DefaultContextCompactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_summary_tokens(mut self, max_summary_tokens: u32) -> Self {
        self.max_summary_tokens = max_summary_tokens.max(1);
        self
    }

    pub fn auto_compact_buffer_tokens(mut self, buffer_tokens: u64) -> Self {
        self.auto_compact_buffer_tokens = buffer_tokens;
        self
    }

    pub fn max_compact_retries(mut self, max_retries: usize) -> Self {
        self.max_compact_retries = max_retries;
        self
    }

    fn auto_compact_threshold(&self, request: &ContextCompactionRequest) -> Option<u64> {
        let max_context_tokens = request.max_context_tokens?;
        let summary_reserve = u64::from(
            request
                .generation_config
                .max_tokens
                .unwrap_or(self.max_summary_tokens)
                .clamp(1, self.max_summary_tokens),
        );
        Some(
            max_context_tokens
                .saturating_sub(summary_reserve)
                .saturating_sub(self.auto_compact_buffer_tokens),
        )
    }
}

impl Default for DefaultContextCompactor {
    fn default() -> Self {
        Self {
            max_summary_tokens: DEFAULT_CONTEXT_COMPACTION_MAX_SUMMARY_TOKENS,
            auto_compact_buffer_tokens: DEFAULT_CONTEXT_COMPACTION_BUFFER_TOKENS,
            max_compact_retries: DEFAULT_CONTEXT_COMPACTION_MAX_RETRIES,
        }
    }
}

#[async_trait]
impl ContextCompactor for DefaultContextCompactor {
    fn name(&self) -> &'static str {
        "default"
    }

    fn should_compact(&self, request: &ContextCompactionRequest) -> bool {
        if request.messages.is_empty() {
            return false;
        }
        match &request.trigger {
            ContextCompactionTrigger::Manual { .. }
            | ContextCompactionTrigger::ContextLengthExceeded { .. } => true,
            ContextCompactionTrigger::Automatic { usage } => self
                .auto_compact_threshold(request)
                .is_some_and(|threshold| usage.used_tokens >= threshold),
        }
    }

    fn prompt(&self, request: &ContextCompactionRequest) -> String {
        let mut prompt = BASE_COMPACT_PROMPT.to_owned();
        if let Some(instructions) = request.trigger.instructions().map(str::trim)
            && !instructions.is_empty()
        {
            prompt.push_str("\n\nAdditional compaction instructions:\n");
            prompt.push_str(instructions);
        }
        prompt
    }

    async fn compact(
        &self,
        provider: &dyn LlmProvider,
        request: ContextCompactionRequest,
        prompt: String,
    ) -> Result<ContextCompactionPlan, ContextCompactionError> {
        if request.messages.is_empty() {
            return Err(ContextCompactionError::new(
                "not enough messages to compact",
            ));
        }

        let mut messages = request
            .messages
            .iter()
            .cloned()
            .map(sanitize_message_for_compaction)
            .collect::<Vec<_>>();
        let mut retries = 0usize;
        let response = loop {
            let response = provider
                .generate(summary_request(
                    &request.generation_config,
                    &messages,
                    &prompt,
                    self.max_summary_tokens,
                ))
                .await;
            match response {
                Ok(response) => break response,
                Err(error)
                    if error.is_context_length_exceeded() && retries < self.max_compact_retries =>
                {
                    retries += 1;
                    messages = truncate_oldest_safe_prefix(&messages).ok_or_else(|| {
                        ContextCompactionError::new(format!(
                            "compaction request still exceeds the context window and no complete older message group can be removed: {error}"
                        ))
                    })?;
                }
                Err(error) => {
                    return Err(ContextCompactionError::new(format!(
                        "conversation summary request failed: {error}"
                    )));
                }
            }
        };

        if !response.message.tool_calls.is_empty() {
            return Err(ContextCompactionError::new(
                "conversation summary response attempted to call a tool",
            ));
        }
        let raw_summary = response
            .message
            .content
            .and_then(Content::into_text)
            .filter(|summary| !summary.trim().is_empty())
            .ok_or_else(|| {
                ContextCompactionError::new("conversation summary response contained no text")
            })?;
        let summary = format_compact_summary(&raw_summary);
        if summary.is_empty() {
            return Err(ContextCompactionError::new(
                "conversation summary was empty after removing drafting content",
            ));
        }

        let continuation = continuation_summary(&summary, request.trigger.is_automatic());
        let replacement = vec![
            Message::system(COMPACT_BOUNDARY_MESSAGE),
            Message::user(continuation),
        ];
        let estimated_context_tokens = estimate_messages_tokens(&replacement);
        Ok(ContextCompactionPlan {
            messages: replacement,
            summary,
            usage: response.usage,
            estimated_context_tokens,
        })
    }
}

fn summary_request(
    generation_config: &GenerationConfig,
    messages: &[Message],
    prompt: &str,
    max_summary_tokens: u32,
) -> ProviderRequest {
    let mut config = generation_config.clone();
    config.temperature = None;
    config.reasoning_effort = Some(ReasoningEffort::None);
    config.max_tokens = Some(
        config
            .max_tokens
            .unwrap_or(max_summary_tokens)
            .clamp(1, max_summary_tokens),
    );

    ProviderRequest {
        messages: std::iter::once(Message::system(SUMMARY_SYSTEM_PROMPT))
            .chain(messages.iter().cloned())
            .chain(std::iter::once(Message::user(prompt)))
            .collect(),
        tools: Vec::new(),
        config,
    }
}

fn sanitize_message_for_compaction(mut message: Message) -> Message {
    message.provider_state = None;
    message.tool_result_metadata = None;
    message.content = message.content.map(|content| match content {
        Content::Text(text) => Content::Text(text),
        Content::Parts(parts) => Content::Parts(
            parts
                .into_iter()
                .map(|part| match part {
                    ContentPart::Text { text } => ContentPart::Text { text },
                    ContentPart::ImageUrl { .. } => ContentPart::text("[image]"),
                    ContentPart::Document { .. } => ContentPart::text("[document]"),
                })
                .collect(),
        ),
    });
    message
}

fn truncate_oldest_safe_prefix(messages: &[Message]) -> Option<Vec<Message>> {
    if messages.len() < 2 {
        return None;
    }

    let target = messages.len().div_ceil(5).max(1);
    let mut pending_tool_results = 0usize;
    let mut safe_cuts = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message.role {
            Role::Assistant => {
                if pending_tool_results != 0 {
                    return None;
                }
                pending_tool_results = message.tool_calls.len();
            }
            Role::Tool => {
                pending_tool_results = pending_tool_results.checked_sub(1)?;
            }
            Role::System | Role::User => {
                if pending_tool_results != 0 {
                    return None;
                }
            }
        }
        let cut = index + 1;
        if pending_tool_results == 0 && cut < messages.len() {
            safe_cuts.push(cut);
        }
    }

    let cut = safe_cuts
        .iter()
        .copied()
        .find(|cut| *cut >= target)
        .or_else(|| safe_cuts.last().copied())?;
    let mut retained = messages[cut..].to_vec();
    if retained
        .first()
        .is_some_and(|message| message.role == Role::Assistant)
    {
        retained.insert(0, Message::user(COMPACT_RETRY_MARKER));
    }
    Some(retained)
}

fn format_compact_summary(raw: &str) -> String {
    let without_analysis = remove_tagged_section(raw, "analysis");
    if let Some(summary) = tagged_section(&without_analysis, "summary") {
        return format!("Summary:\n{}", summary.trim());
    }
    without_analysis.trim().to_owned()
}

fn remove_tagged_section(value: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = value.find(&open) else {
        return value.to_owned();
    };
    let Some(relative_end) = value[start + open.len()..].find(&close) else {
        return value.to_owned();
    };
    let end = start + open.len() + relative_end + close.len();
    let mut result = value.to_owned();
    result.replace_range(start..end, "");
    result
}

fn tagged_section<'a>(value: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = value.find(&open)? + open.len();
    let end = value[start..].find(&close)? + start;
    Some(&value[start..end])
}

fn continuation_summary(summary: &str, suppress_follow_up_questions: bool) -> String {
    let mut message = format!(
        "This session is being continued from an earlier conversation that reached its context limit. The summary below is the durable context for the earlier conversation.\n\n{summary}"
    );
    if suppress_follow_up_questions {
        message.push_str(
            "\n\nContinue from where the conversation stopped. Do not ask the user to repeat information, do not recap or acknowledge this summary, and do not preface the continuation. Resume the latest task directly as if the context boundary had not occurred.",
        );
    }
    message
}

pub(crate) fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|message| {
            let content_bytes = match &message.content {
                Some(Content::Text(text)) => text.len(),
                Some(Content::Parts(parts)) => parts
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text } => text.len(),
                        ContentPart::ImageUrl { .. } => 6_400,
                        ContentPart::Document { document } => document.data.len().min(200_000),
                    })
                    .sum(),
                None => 0,
            };
            let tool_bytes =
                serde_json::to_string(&message.tool_calls).map_or(0, |serialized| serialized.len());
            u64::try_from((content_bytes + tool_bytes).div_ceil(4).saturating_add(4))
                .unwrap_or(u64::MAX)
        })
        .fold(0u64, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::{
        error::ProviderError,
        provider::ProviderEventStream,
        types::{
            AssistantMessage, ContextUsage, ImageUrl, ProviderEvent, ProviderResponse, ToolCall,
        },
    };

    struct QueueProvider {
        results: Mutex<VecDeque<Result<ProviderResponse, ProviderError>>>,
        requests: Arc<Mutex<Vec<ProviderRequest>>>,
    }

    impl LlmProvider for QueueProvider {
        fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
            self.requests.lock().unwrap().push(request);
            let result = self.results.lock().unwrap().pop_front().unwrap();
            Box::pin(futures_util::stream::iter(
                [result.map(ProviderEvent::Done)],
            ))
        }
    }

    fn request(trigger: ContextCompactionTrigger) -> ContextCompactionRequest {
        ContextCompactionRequest {
            trigger,
            system_prompt: "system".to_owned(),
            messages: vec![Message::user("hello")],
            max_context_tokens: Some(200_000),
            generation_config: GenerationConfig {
                max_tokens: Some(20_000),
                ..GenerationConfig::default()
            },
        }
    }

    #[test]
    fn threshold_reserves_summary_output_and_fixed_headroom() {
        assert_eq!(
            default_context_compaction_threshold(200_000, Some(20_000)),
            167_000
        );
        assert_eq!(
            default_context_compaction_threshold(200_000, Some(8_000)),
            179_000
        );

        let compactor = DefaultContextCompactor::default();
        assert!(
            !compactor.should_compact(&request(ContextCompactionTrigger::Automatic {
                usage: ContextUsage {
                    max_tokens: 200_000,
                    used_tokens: 166_999,
                    remaining_tokens: 33_001,
                },
            }))
        );
        assert!(
            compactor.should_compact(&request(ContextCompactionTrigger::Automatic {
                usage: ContextUsage {
                    max_tokens: 200_000,
                    used_tokens: 167_000,
                    remaining_tokens: 33_000,
                },
            }))
        );
    }

    #[test]
    fn prompt_includes_manual_instructions() {
        let compactor = DefaultContextCompactor::default();
        let prompt = compactor.prompt(&request(ContextCompactionTrigger::Manual {
            instructions: Some("Focus on storage invariants".to_owned()),
        }));
        assert!(prompt.contains("Focus on storage invariants"));
        assert!(prompt.contains("<summary>"));
    }

    #[test]
    fn formatting_removes_analysis_and_keeps_summary() {
        assert_eq!(
            format_compact_summary(
                "<analysis>draft only</analysis>\n<summary>durable facts</summary>"
            ),
            "Summary:\ndurable facts"
        );
    }

    #[test]
    fn sanitizing_replaces_media_and_provider_state_is_removed() {
        let message = Message::user_parts([
            ContentPart::text("look"),
            ContentPart::image(ImageUrl::new("data:image/png;base64,secret")),
        ]);
        let sanitized = sanitize_message_for_compaction(message);
        assert_eq!(
            sanitized.content,
            Some(Content::parts([
                ContentPart::text("look"),
                ContentPart::text("[image]")
            ]))
        );
        assert!(sanitized.provider_state.is_none());
    }

    #[tokio::test]
    async fn prompt_too_long_retry_drops_only_complete_protocol_groups() {
        let source = vec![
            Message::user("old user"),
            Message::assistant(Some(Content::text("old answer")), Vec::new()),
            Message::user("tool request"),
            Message::assistant(
                None,
                vec![ToolCall::new("call-1", "read", serde_json::json!({}))],
            ),
            Message::tool_result("call-1", "tool result", false),
            Message::user("latest user"),
            Message::assistant(Some(Content::text("latest answer")), Vec::new()),
        ];
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = QueueProvider {
            results: Mutex::new(VecDeque::from([
                Err(ProviderError::context_length_exceeded("summary too long")),
                Ok(ProviderResponse {
                    message: AssistantMessage::text("<summary>durable summary</summary>"),
                    usage: None,
                }),
            ])),
            requests: Arc::clone(&requests),
        };
        let request = ContextCompactionRequest {
            trigger: ContextCompactionTrigger::Manual { instructions: None },
            system_prompt: "system".to_owned(),
            messages: source.clone(),
            max_context_tokens: Some(200_000),
            generation_config: GenerationConfig::default(),
        };
        let compactor = DefaultContextCompactor::default();

        let plan = compactor
            .compact(&provider, request.clone(), compactor.prompt(&request))
            .await
            .unwrap();

        assert!(plan.summary.contains("durable summary"));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(&requests[0].messages[1..8], source.as_slice());
        assert_eq!(&requests[1].messages[1..6], &source[2..]);
        assert_eq!(requests[1].messages[2].tool_calls[0].id, "call-1");
        assert_eq!(
            requests[1].messages[3].tool_call_id.as_deref(),
            Some("call-1")
        );
    }
}
