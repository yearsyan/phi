use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    time::Duration,
};

use async_trait::async_trait;
use phi::{
    Agent, AgentEvent, AssistantDelta, DEFAULT_MAX_RETRIES, DiskSessionStorage, OpenAiChatProvider,
    ReasoningEffort, RetryConfig, Tool, ToolDefinition, ToolError, ToolOutput,
};
use serde_json::json;

struct CharacterCount;

#[async_trait]
impl Tool for CharacterCount {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "character_count",
            "Count the Unicode characters in the supplied text.",
            json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text whose characters should be counted"
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let text = arguments["text"]
            .as_str()
            .ok_or_else(|| ToolError::new("text must be a string"))?;
        let result = json!({
            "text": text,
            "characters": text.chars().count()
        });
        Ok(ToolOutput::success(result.to_string()))
    }
}

fn load_api_key() -> Result<String, Box<dyn Error>> {
    dotenvy::dotenv().ok();

    env::var("LLM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            fs::read_to_string(".dskey")
                .ok()
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty())
        })
        .ok_or_else(|| "missing API key; set LLM_API_KEY, create .env, or add it to .dskey".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();
    let max_retries = env::var("LLM_MAX_RETRIES")
        .unwrap_or_else(|_| DEFAULT_MAX_RETRIES.to_string())
        .parse()?;
    let request_timeout_secs = env::var("LLM_REQUEST_TIMEOUT_SECS")
        .unwrap_or_else(|_| "30".to_owned())
        .parse()?;
    let retry_config = RetryConfig::default()
        .with_max_retries(max_retries)
        .with_request_timeout(Duration::from_secs(request_timeout_secs));
    let provider = OpenAiChatProvider::new(
        load_api_key()?,
        env::var("LLM_BASE_URL")?,
        env::var("LLM_MODEL")?,
    )?
    .retry_config(retry_config);
    let max_context_tokens = env::var("LLM_MAX_CONTEXT_TOKENS")?.parse()?;
    let max_output_tokens = env::var("LLM_MAX_OUTPUT_TOKENS")
        .unwrap_or_else(|_| "4096".to_owned())
        .parse()?;
    let mut builder = Agent::builder(provider)
        .system_prompt(
            "You are a concise assistant. When a relevant tool is available, use it before answering.",
        )
        .tool(CharacterCount)
        .max_turns(8)
        .max_tokens(max_output_tokens)
        .max_context_tokens(max_context_tokens);
    if let Ok(temperature) = env::var("LLM_TEMPERATURE") {
        builder = builder.temperature(temperature.parse()?);
    }
    if let Ok(reasoning_effort) = env::var("LLM_REASONING_EFFORT") {
        let reasoning_effort = match reasoning_effort.as_str() {
            "none" => ReasoningEffort::None,
            "minimal" => ReasoningEffort::Minimal,
            "low" => ReasoningEffort::Low,
            "medium" => ReasoningEffort::Medium,
            "high" => ReasoningEffort::High,
            "xhigh" => ReasoningEffort::XHigh,
            "max" => ReasoningEffort::Max,
            value => return Err(format!(
                "invalid LLM_REASONING_EFFORT {value:?}; expected none, minimal, low, medium, high, xhigh, or max"
            ).into()),
        };
        builder = builder.reasoning_effort(reasoning_effort);
    }
    let mut agent = builder.build();
    if let Ok(session_id) = env::var("LLM_SESSION_ID") {
        let session_dir =
            env::var("LLM_SESSION_DIR").unwrap_or_else(|_| ".phi/sessions".to_owned());
        agent
            .attach_session(session_id, DiskSessionStorage::new(session_dir))
            .await?;
        eprintln!("[session] restored_messages={}", agent.messages().len());
    }

    agent.subscribe(|event| match event {
        AgentEvent::TurnStart { turn } => eprintln!("[turn {turn}]"),
        AgentEvent::ToolExecutionStart { call } => {
            eprintln!("[tool start] {}", call.name)
        }
        AgentEvent::ToolExecutionEnd { call, is_error, .. } => {
            eprintln!("[tool end] {} error={is_error}", call.name)
        }
        AgentEvent::MessageUpdate {
            delta: AssistantDelta::Text { delta },
        } => {
            print!("{delta}");
            io::stdout().flush().expect("flush stdout");
        }
        AgentEvent::UsageUpdate {
            usage,
            context_usage,
        } => {
            if let Some(context) = context_usage {
                eprintln!(
                    "[context] used={} remaining={} max={}",
                    context.used_tokens, context.remaining_tokens, context.max_tokens
                );
            } else {
                eprintln!("[usage] total={}", usage.total_tokens);
            }
        }
        AgentEvent::ProviderRetry { event } => {
            eprintln!(
                "[provider retry {}/{} in {:?}] {:?}",
                event.retry_number, event.max_retries, event.delay, event.reason
            );
        }
        _ => {}
    });

    let prompt = env::args().nth(1).unwrap_or_else(|| {
        "请调用 character_count 工具统计“Rust智能体”有多少个字符，然后用中文回答。".to_owned()
    });
    let result = agent.prompt(prompt).await?;

    println!();
    if result.text().is_none() {
        println!("<no text response>");
    }
    eprintln!("[run usage] total={}", result.run_usage.total_tokens);
    Ok(())
}
