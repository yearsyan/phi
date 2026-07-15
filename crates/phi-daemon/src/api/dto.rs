use std::{fmt, time::Duration};

use phi::{
    AgentEvent, AgentMode, AssistantDelta, Content, ContentPart, ContextUsage, Message,
    ProviderRetryReason, ReasoningEffort, Role, SkillDiagnostic, SkillInvocation, SkillMetadata,
    TokenUsage, ToolCall, ToolProgress,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    runtime::{
        AgentStatus, AgentSummary, AgentView, AskUserAnswer, AskUserId, AskUserRequest,
        AssistantDraft, PlanApprovalDecision, PlanApprovalId, PlanApprovalRequest, RunId,
        RuntimeEvent, RuntimeEventKind, SessionId, ToolCallDraft,
    },
    service::SessionListing,
    store::{
        DEFAULT_MAX_RETRIES, DEFAULT_REQUEST_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ProviderConfig,
        ProviderKind, ProviderProfile,
    },
};

#[derive(Deserialize)]
pub struct PutProviderRequest {
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    pub max_context_tokens: u64,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
}

impl fmt::Debug for PutProviderRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PutProviderRequest")
            .field("provider", &self.provider)
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("system_prompt", &self.system_prompt)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("max_context_tokens", &self.max_context_tokens)
            .field("temperature", &self.temperature)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("max_retries", &self.max_retries)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("stream_idle_timeout_secs", &self.stream_idle_timeout_secs)
            .finish()
    }
}

impl From<PutProviderRequest> for ProviderConfig {
    fn from(request: PutProviderRequest) -> Self {
        Self {
            provider: request.provider,
            api_key: request.api_key,
            base_url: request.base_url,
            model: request.model,
            system_prompt: request.system_prompt,
            max_output_tokens: request.max_output_tokens,
            max_context_tokens: request.max_context_tokens,
            temperature: request.temperature,
            reasoning_effort: request.reasoning_effort,
            max_retries: request.max_retries,
            request_timeout_secs: request.request_timeout_secs,
            stream_idle_timeout_secs: request.stream_idle_timeout_secs,
            revision: 0,
        }
    }
}

fn default_max_retries() -> usize {
    DEFAULT_MAX_RETRIES
}

fn default_request_timeout_secs() -> u64 {
    DEFAULT_REQUEST_TIMEOUT.as_secs()
}

fn default_stream_idle_timeout_secs() -> u64 {
    DEFAULT_STREAM_IDLE_TIMEOUT.as_secs()
}

#[derive(Debug, Serialize)]
pub struct ProviderResponse {
    pub configured: bool,
    pub provider: Option<PublicProviderConfig>,
}

impl ProviderResponse {
    pub fn from_config(profile_id: &str, config: Option<&ProviderConfig>) -> Self {
        Self {
            configured: config.is_some(),
            provider: config.map(|config| PublicProviderConfig::new(profile_id, config)),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PublicProviderConfig {
    pub profile_id: String,
    pub provider: ProviderKind,
    pub api_key_configured: bool,
    pub base_url: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub max_context_tokens: u64,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_retries: usize,
    pub request_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
    pub revision: u64,
}

impl PublicProviderConfig {
    fn new(profile_id: &str, config: &ProviderConfig) -> Self {
        Self {
            profile_id: profile_id.to_owned(),
            provider: config.provider,
            api_key_configured: !config.api_key.is_empty(),
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            system_prompt: config.system_prompt.clone(),
            max_output_tokens: config.max_output_tokens,
            max_context_tokens: config.max_context_tokens,
            temperature: config.temperature,
            reasoning_effort: config.reasoning_effort,
            max_retries: config.max_retries,
            request_timeout_secs: config.request_timeout_secs,
            stream_idle_timeout_secs: config.stream_idle_timeout_secs,
            revision: config.revision,
        }
    }
}

impl From<&ProviderProfile> for PublicProviderConfig {
    fn from(profile: &ProviderProfile) -> Self {
        Self::new(&profile.profile_id, &profile.config)
    }
}

#[derive(Debug, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<PublicProviderConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Prompt {
        request_id: String,
        content: Content,
        #[serde(default)]
        skill: Option<SkillInvocation>,
    },
    Stop {
        request_id: String,
        run_id: RunId,
    },
    SetModel {
        request_id: String,
        model: String,
    },
    SetReasoningEffort {
        request_id: String,
        effort: Option<ReasoningEffort>,
    },
    SetMode {
        request_id: String,
        mode: AgentMode,
    },
    #[serde(rename = "answer_askuser")]
    AnswerAskUser {
        request_id: String,
        ask_id: AskUserId,
        answers: Vec<AskUserAnswer>,
    },
    DecidePlanApproval {
        request_id: String,
        approval_id: PlanApprovalId,
        decision: PlanApprovalDecision,
    },
    Ping {
        request_id: String,
    },
}

impl ClientCommand {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Prompt { request_id, .. }
            | Self::Stop { request_id, .. }
            | Self::SetModel { request_id, .. }
            | Self::SetReasoningEffort { request_id, .. }
            | Self::SetMode { request_id, .. }
            | Self::AnswerAskUser { request_id, .. }
            | Self::DecidePlanApproval { request_id, .. }
            | Self::Ping { request_id } => request_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Building,
    Ready {
        config: SessionConfigDto,
        mode: AgentMode,
    },
    SessionCreated {
        session_id: SessionId,
    },
    Snapshot {
        session: SessionDto,
    },
    CommandAccepted {
        request_id: String,
        command: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_position: Option<usize>,
    },
    CommandRejected {
        request_id: String,
        code: &'static str,
        message: String,
    },
    Event {
        sequence: u64,
        session_id: SessionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        event: EventDto,
    },
    ResyncRequired {
        skipped: u64,
        session: SessionDto,
    },
    Pong {
        request_id: String,
    },
    FatalError {
        code: &'static str,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionDto {
    pub session_id: SessionId,
    pub profile_id: String,
    pub initialized: bool,
    pub status: SessionStatusDto,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub mode: AgentMode,
    pub config: SessionConfigDto,
    pub history: Vec<PublicMessage>,
    pub draft: Option<AssistantDraftDto>,
    pub pending_asks: Vec<AskUserRequest>,
    pub pending_plan_approvals: Vec<PlanApprovalRequest>,
    pub usage: UsageDto,
    pub last_sequence: u64,
}

impl From<&AgentView> for SessionDto {
    fn from(view: &AgentView) -> Self {
        Self {
            session_id: view.session_id,
            profile_id: view.profile_id.clone(),
            initialized: view.initialized,
            status: view.status.into(),
            active_run_id: view.active_run_id,
            queued_runs: view.queued_runs,
            mode: view.mode,
            config: SessionConfigDto {
                model: view.model.clone(),
                reasoning_effort: view.reasoning_effort,
                revision: view.config_revision,
            },
            history: view.messages.iter().map(PublicMessage::from).collect(),
            draft: view.draft.as_ref().map(AssistantDraftDto::from),
            pending_asks: view.pending_asks.clone(),
            pending_plan_approvals: view.pending_plan_approvals.clone(),
            usage: UsageDto {
                last: view.last_usage,
                context: view.context_usage.map(ContextUsageDto::from),
                cumulative: view.cumulative_usage,
            },
            last_sequence: view.last_event_sequence,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionConfigDto {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub revision: u64,
}

impl SessionConfigDto {
    pub fn from_summary(summary: &AgentSummary) -> Self {
        Self {
            model: summary.model.clone(),
            reasoning_effort: summary.reasoning_effort,
            revision: summary.config_revision,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusDto {
    AwaitingFirstPrompt,
    Idle,
    Running,
    Stopping,
    Closing,
    Closed,
    Offline,
}

impl From<AgentStatus> for SessionStatusDto {
    fn from(value: AgentStatus) -> Self {
        match value {
            AgentStatus::AwaitingFirstPrompt => Self::AwaitingFirstPrompt,
            AgentStatus::Idle => Self::Idle,
            AgentStatus::Running => Self::Running,
            AgentStatus::Stopping => Self::Stopping,
            AgentStatus::Closing => Self::Closing,
            AgentStatus::Closed => Self::Closed,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicMessage {
    pub role: Role,
    pub content: Option<Content>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub tool_result_is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_metadata: Option<Value>,
}

impl From<&Message> for PublicMessage {
    fn from(message: &Message) -> Self {
        Self {
            role: message.role.clone(),
            content: message.content.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            tool_result_is_error: message.tool_result_is_error,
            tool_result_metadata: message.tool_result_metadata.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantDraftDto {
    pub text: String,
    pub tool_calls: Vec<ToolCallDraftDto>,
}

impl From<&AssistantDraft> for AssistantDraftDto {
    fn from(draft: &AssistantDraft) -> Self {
        Self {
            text: draft.text.clone(),
            tool_calls: draft
                .tool_calls
                .iter()
                .map(ToolCallDraftDto::from)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCallDraftDto {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

impl From<&ToolCallDraft> for ToolCallDraftDto {
    fn from(tool_call: &ToolCallDraft) -> Self {
        Self {
            index: tool_call.index,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageDto {
    pub last: Option<TokenUsage>,
    pub context: Option<ContextUsageDto>,
    pub cumulative: TokenUsage,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ContextUsageDto {
    pub max_tokens: u64,
    pub used_tokens: u64,
    pub remaining_tokens: u64,
}

impl From<ContextUsage> for ContextUsageDto {
    fn from(value: ContextUsage) -> Self {
        Self {
            max_tokens: value.max_tokens,
            used_tokens: value.used_tokens,
            remaining_tokens: value.remaining_tokens,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SessionsResponse {
    pub sessions: Vec<SessionSummaryDto>,
}

#[derive(Debug, Serialize)]
pub struct SkillsResponse {
    pub session_id: SessionId,
    pub skills: Vec<SkillSummaryDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<SkillDiagnosticDto>,
}

#[derive(Debug, Serialize)]
pub struct SkillSummaryDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl From<&SkillMetadata> for SkillSummaryDto {
    fn from(skill: &SkillMetadata) -> Self {
        Self {
            name: skill.name.clone(),
            display_name: skill.display_name.clone(),
            description: skill.description.clone(),
            when_to_use: skill.when_to_use.clone(),
            argument_hint: skill.argument_hint.clone(),
            arguments: skill.argument_names.clone(),
            version: skill.version.clone(),
            model_invocable: skill.model_invocable,
            user_invocable: skill.user_invocable,
            source: skill.source.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SkillDiagnosticDto {
    pub level: phi::DiagnosticLevel,
    pub code: String,
    pub message: String,
}

impl From<&SkillDiagnostic> for SkillDiagnosticDto {
    fn from(diagnostic: &SkillDiagnostic) -> Self {
        Self {
            level: diagnostic.level,
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SessionSummaryDto {
    pub session_id: SessionId,
    pub profile_id: String,
    pub status: SessionStatusDto,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub mode: Option<AgentMode>,
    pub config: SessionConfigDto,
    pub message_count: Option<usize>,
}

impl From<SessionListing> for SessionSummaryDto {
    fn from(listing: SessionListing) -> Self {
        match listing.state {
            Some(state) => Self {
                session_id: state.session_id,
                profile_id: state.profile_id,
                status: state.status.into(),
                active_run_id: state.active_run_id,
                queued_runs: state.queued_runs,
                mode: Some(state.mode),
                config: SessionConfigDto {
                    model: state.model,
                    reasoning_effort: state.reasoning_effort,
                    revision: state.config_revision,
                },
                message_count: Some(state.message_count),
            },
            None => Self {
                session_id: listing.record.id,
                profile_id: listing.record.profile_id,
                status: SessionStatusDto::Offline,
                active_run_id: None,
                queued_runs: 0,
                mode: None,
                config: SessionConfigDto {
                    model: listing.record.model,
                    reasoning_effort: listing.record.reasoning_effort,
                    revision: listing.record.config_revision,
                },
                message_count: None,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventDto {
    StateChanged {
        status: SessionStatusDto,
    },
    SessionInitialized,
    RunQueued {
        run_id: RunId,
    },
    RunStarted {
        run_id: RunId,
    },
    RunCompleted {
        run_id: RunId,
    },
    RunStopped {
        run_id: RunId,
    },
    RunFailed {
        run_id: RunId,
        message: String,
    },
    ConfigChanged {
        config: SessionConfigDto,
    },
    ModeChanged {
        mode: AgentMode,
    },
    #[serde(rename = "askuser_requested")]
    AskUserRequested {
        request: AskUserRequest,
    },
    #[serde(rename = "askuser_answered")]
    AskUserAnswered {
        ask_id: AskUserId,
    },
    #[serde(rename = "askuser_cancelled")]
    AskUserCancelled {
        ask_id: AskUserId,
    },
    PlanApprovalRequested {
        request: PlanApprovalRequest,
    },
    PlanApprovalDecided {
        approval_id: PlanApprovalId,
        decision: PlanApprovalDecision,
    },
    PlanApprovalCancelled {
        approval_id: PlanApprovalId,
    },
    OperationFailed {
        operation: String,
        message: String,
    },
    ActorCrashed {
        message: String,
    },
    AgentStart,
    AgentEnd,
    AgentStopped,
    TurnStart {
        turn: usize,
    },
    TurnEnd {
        turn: usize,
        message: PublicMessage,
        tool_results: Vec<PublicMessage>,
    },
    MessageStart {
        message: PublicMessage,
    },
    MessageUpdate {
        delta: DeltaDto,
    },
    MessageEnd {
        message: PublicMessage,
    },
    MessageAborted,
    ToolExecutionStart {
        call: ToolCall,
    },
    ToolExecutionProgress {
        call: ToolCall,
        progress: ToolProgress,
    },
    ToolExecutionEnd {
        call: ToolCall,
        content: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        content_parts: Vec<ContentPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    UsageUpdate {
        usage: TokenUsage,
        context_usage: Option<ContextUsageDto>,
    },
    ProviderRetry {
        retry_number: usize,
        max_retries: usize,
        delay_ms: u64,
        reason: RetryReasonDto,
    },
    Error {
        message: String,
    },
}

impl From<RuntimeEvent> for ServerMessage {
    fn from(event: RuntimeEvent) -> Self {
        Self::Event {
            sequence: event.sequence,
            session_id: event.session_id,
            run_id: event.run_id,
            event: EventDto::from(event.kind),
        }
    }
}

impl From<RuntimeEventKind> for EventDto {
    fn from(event: RuntimeEventKind) -> Self {
        match event {
            RuntimeEventKind::StateChanged { status } => Self::StateChanged {
                status: status.into(),
            },
            RuntimeEventKind::SessionInitialized => Self::SessionInitialized,
            RuntimeEventKind::RunQueued { run_id } => Self::RunQueued { run_id },
            RuntimeEventKind::RunStarted { run_id } => Self::RunStarted { run_id },
            RuntimeEventKind::RunCompleted { run_id } => Self::RunCompleted { run_id },
            RuntimeEventKind::RunStopped { run_id } => Self::RunStopped { run_id },
            RuntimeEventKind::RunFailed { run_id, message } => Self::RunFailed { run_id, message },
            RuntimeEventKind::ConfigChanged {
                model,
                reasoning_effort,
                revision,
            } => Self::ConfigChanged {
                config: SessionConfigDto {
                    model,
                    reasoning_effort,
                    revision,
                },
            },
            RuntimeEventKind::ModeChanged { mode } => Self::ModeChanged { mode },
            RuntimeEventKind::AskUserRequested { request } => Self::AskUserRequested { request },
            RuntimeEventKind::AskUserAnswered { ask_id } => Self::AskUserAnswered { ask_id },
            RuntimeEventKind::AskUserCancelled { ask_id } => Self::AskUserCancelled { ask_id },
            RuntimeEventKind::PlanApprovalRequested { request } => {
                Self::PlanApprovalRequested { request }
            }
            RuntimeEventKind::PlanApprovalDecided {
                approval_id,
                decision,
            } => Self::PlanApprovalDecided {
                approval_id,
                decision,
            },
            RuntimeEventKind::PlanApprovalCancelled { approval_id } => {
                Self::PlanApprovalCancelled { approval_id }
            }
            RuntimeEventKind::OperationFailed { operation, message } => {
                Self::OperationFailed { operation, message }
            }
            RuntimeEventKind::ActorCrashed { message } => Self::ActorCrashed { message },
            RuntimeEventKind::Agent(event) => Self::from(event),
        }
    }
}

impl From<AgentEvent> for EventDto {
    fn from(event: AgentEvent) -> Self {
        match event {
            AgentEvent::AgentStart => Self::AgentStart,
            AgentEvent::AgentEnd { .. } => Self::AgentEnd,
            AgentEvent::AgentStopped { .. } => Self::AgentStopped,
            AgentEvent::TurnStart { turn } => Self::TurnStart { turn },
            AgentEvent::TurnEnd {
                turn,
                message,
                tool_results,
            } => Self::TurnEnd {
                turn,
                message: PublicMessage::from(&message),
                tool_results: tool_results.iter().map(PublicMessage::from).collect(),
            },
            AgentEvent::MessageStart { message } => Self::MessageStart {
                message: PublicMessage::from(&message),
            },
            AgentEvent::MessageUpdate { delta } => Self::MessageUpdate {
                delta: delta.into(),
            },
            AgentEvent::MessageEnd { message } => Self::MessageEnd {
                message: PublicMessage::from(&message),
            },
            AgentEvent::MessageAborted => Self::MessageAborted,
            AgentEvent::ToolExecutionStart { call } => Self::ToolExecutionStart { call },
            AgentEvent::ToolExecutionProgress { call, progress } => {
                Self::ToolExecutionProgress { call, progress }
            }
            AgentEvent::ToolExecutionEnd {
                call,
                content,
                is_error,
                content_parts,
                metadata,
            } => Self::ToolExecutionEnd {
                call,
                content,
                is_error,
                content_parts,
                metadata,
            },
            AgentEvent::UsageUpdate {
                usage,
                context_usage,
            } => Self::UsageUpdate {
                usage,
                context_usage: context_usage.map(ContextUsageDto::from),
            },
            AgentEvent::ProviderRetry { event } => Self::ProviderRetry {
                retry_number: event.retry_number,
                max_retries: event.max_retries,
                delay_ms: duration_millis(event.delay),
                reason: event.reason.into(),
            },
            AgentEvent::Error { message } => Self::Error { message },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeltaDto {
    Text {
        delta: String,
    },
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
}

impl From<AssistantDelta> for DeltaDto {
    fn from(delta: AssistantDelta) -> Self {
        match delta {
            AssistantDelta::Text { delta } => Self::Text { delta },
            AssistantDelta::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => Self::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RetryReasonDto {
    RequestTimeout { timeout_ms: u64 },
    Transport { message: String },
    HttpStatus { status: u16, body: String },
}

impl From<ProviderRetryReason> for RetryReasonDto {
    fn from(reason: ProviderRetryReason) -> Self {
        match reason {
            ProviderRetryReason::RequestTimeout { timeout } => Self::RequestTimeout {
                timeout_ms: duration_millis(timeout),
            },
            ProviderRetryReason::Transport { message } => Self::Transport { message },
            ProviderRetryReason::HttpStatus { status, body } => Self::HttpStatus { status, body },
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_request_debug_redacts_api_key() {
        let secret = "canary-provider-key-that-must-never-appear";
        let request: PutProviderRequest = serde_json::from_value(serde_json::json!({
            "provider": "openai_chat",
            "api_key": secret,
            "base_url": "https://example.test/v1",
            "model": "test-model",
            "max_context_tokens": 128000
        }))
        .unwrap();

        assert_eq!(request.request_timeout_secs, 30);
        assert_eq!(request.stream_idle_timeout_secs, 120);
        let debug = format!("{request:?}");
        assert!(!debug.contains(secret));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn provider_request_requires_max_context_tokens() {
        let missing = serde_json::from_value::<PutProviderRequest>(serde_json::json!({
            "provider": "openai_chat",
            "api_key": "secret",
            "base_url": "https://example.test/v1",
            "model": "test-model"
        }));
        assert!(missing.is_err());

        let null = serde_json::from_value::<PutProviderRequest>(serde_json::json!({
            "provider": "openai_chat",
            "api_key": "secret",
            "base_url": "https://example.test/v1",
            "model": "test-model",
            "max_context_tokens": null
        }));
        assert!(null.is_err());
    }

    #[test]
    fn prompt_skill_is_optional_and_backward_compatible() {
        let legacy: ClientCommand = serde_json::from_value(serde_json::json!({
            "type": "prompt",
            "request_id": "legacy",
            "content": { "type": "text", "value": "hello" }
        }))
        .unwrap();
        assert!(matches!(legacy, ClientCommand::Prompt { skill: None, .. }));

        let selected: ClientCommand = serde_json::from_value(serde_json::json!({
            "type": "prompt",
            "request_id": "selected",
            "content": { "type": "text", "value": "review this" },
            "skill": { "name": "review", "arguments": "--security" }
        }))
        .unwrap();
        assert!(matches!(
            selected,
            ClientCommand::Prompt { skill: Some(skill), .. }
                if skill.name == "review" && skill.arguments.as_deref() == Some("--security")
        ));
    }
}
