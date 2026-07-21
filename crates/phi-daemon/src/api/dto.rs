use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
    time::Duration,
};

use chrono::{DateTime, Utc};
use phi::{
    ActiveSubagentRun, AgentEvent, AssistantDelta, CapabilityMode, Content, ContentPart,
    ContextCompactionTrigger, ContextUsage, EffectiveSubagentConfig, Message, MessageVisibility,
    ProviderRetryReason, ProviderState, ReasoningEffort, Role, SkillDiagnostic, SkillInvocation,
    SkillMetadata, SubagentEvent, SubagentEventKind, SubagentNotification,
    SubagentResourceFinalization, SubagentResourceInfo, SubagentRunOutcome, SubagentSnapshot,
    SubagentState, TokenUsage, ToolCall, ToolProgress, ValidatedSubagentOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    runtime::{
        AgentProfile, AgentProfileDefinition, AgentStatus, AgentSummary, AgentView, AskUserAnswer,
        AskUserId, AskUserRequest, AssistantDraft, ContextCompactionPhase, ContextCompactionView,
        NamePolicy, PromptDefinition, PromptMode, RunId, RuntimeEvent, RuntimeEventKind, SessionId,
        SubagentSummary, ToolCallDraft,
    },
    scheduled_task::{
        CreateScheduledTask, ScheduledIntervalUnit, ScheduledRunOutcome, ScheduledTask,
        ScheduledTaskId, ScheduledTaskRun, ScheduledTaskSchedule, ScheduledWeekday,
        UpdateScheduledTask,
    },
    service::{ForkPosition, SessionListing},
    store::{
        DEFAULT_MAX_RETRIES, DEFAULT_REQUEST_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ProviderConfig,
        ProviderKind, ProviderProfile,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduledTaskRequest {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    #[serde(default)]
    pub capability_mode: Option<CapabilityMode>,
    pub schedule: ScheduledTaskScheduleDto,
}

impl fmt::Debug for CreateScheduledTaskRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateScheduledTaskRequest")
            .field("name", &self.name)
            .field("prompt", &"[REDACTED]")
            .field("workspace", &self.workspace)
            .field("profile_id", &self.profile_id)
            .field("agent_profile_id", &self.agent_profile_id)
            .field("capability_mode", &self.capability_mode)
            .field("schedule", &self.schedule)
            .finish()
    }
}

impl CreateScheduledTaskRequest {
    pub fn into_create(
        self,
        workspace: phi::Workspace,
        default_profile_id: &str,
        default_agent_profile_id: &str,
    ) -> CreateScheduledTask {
        CreateScheduledTask {
            name: self.name,
            prompt: self.prompt,
            workspace,
            profile_id: self
                .profile_id
                .unwrap_or_else(|| default_profile_id.to_owned()),
            agent_profile_id: self
                .agent_profile_id
                .unwrap_or_else(|| default_agent_profile_id.to_owned()),
            capability_mode: self.capability_mode,
            schedule: self.schedule.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledTaskScheduleDto {
    Daily {
        time: String,
        weekdays: Vec<ScheduledWeekday>,
        timezone: String,
    },
    Interval {
        every: u32,
        unit: ScheduledIntervalUnit,
    },
}

impl From<ScheduledTaskScheduleDto> for ScheduledTaskSchedule {
    fn from(schedule: ScheduledTaskScheduleDto) -> Self {
        match schedule {
            ScheduledTaskScheduleDto::Daily {
                time,
                weekdays,
                timezone,
            } => Self::Daily {
                time,
                weekdays,
                timezone,
            },
            ScheduledTaskScheduleDto::Interval { every, unit } => Self::Interval { every, unit },
        }
    }
}

impl From<&ScheduledTaskSchedule> for ScheduledTaskScheduleDto {
    fn from(schedule: &ScheduledTaskSchedule) -> Self {
        match schedule {
            ScheduledTaskSchedule::Daily {
                time,
                weekdays,
                timezone,
            } => Self::Daily {
                time: time.clone(),
                weekdays: weekdays.clone(),
                timezone: timezone.clone(),
            },
            ScheduledTaskSchedule::Interval { every, unit } => Self::Interval {
                every: *every,
                unit: *unit,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateScheduledTaskRequest {
    pub enabled: bool,
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

impl From<UpdateScheduledTaskRequest> for UpdateScheduledTask {
    fn from(request: UpdateScheduledTaskRequest) -> Self {
        Self {
            enabled: request.enabled,
            expected_revision: request.expected_revision,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ScheduledTasksResponse {
    pub tasks: Vec<ScheduledTaskDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScheduledTaskDto {
    pub task_id: ScheduledTaskId,
    pub name: String,
    pub prompt: String,
    pub workspace: String,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub capability_mode: Option<CapabilityMode>,
    pub schedule: ScheduledTaskScheduleDto,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run: Option<ScheduledTaskRunDto>,
    pub skipped_runs: u64,
    pub revision: u64,
}

impl From<ScheduledTask> for ScheduledTaskDto {
    fn from(task: ScheduledTask) -> Self {
        Self {
            task_id: task.id,
            name: task.name,
            prompt: task.prompt,
            workspace: task.workspace.to_string(),
            profile_id: task.profile_id,
            agent_profile_id: task.agent_profile_id,
            capability_mode: task.capability_mode,
            schedule: ScheduledTaskScheduleDto::from(&task.schedule),
            enabled: task.enabled,
            created_at: task.created_at,
            updated_at: task.updated_at,
            next_run_at: task.next_run_at,
            last_run: task.last_run.map(ScheduledTaskRunDto::from),
            skipped_runs: task.skipped_runs,
            revision: task.revision,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ScheduledTaskRunDto {
    pub scheduled_for: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: ScheduledRunOutcome,
    pub session_id: Option<SessionId>,
    pub error: Option<String>,
}

impl From<ScheduledTaskRun> for ScheduledTaskRunDto {
    fn from(run: ScheduledTaskRun) -> Self {
        Self {
            scheduled_for: run.scheduled_for,
            started_at: run.started_at,
            finished_at: run.finished_at,
            outcome: run.outcome,
            session_id: run.session_id,
            error: run.error,
        }
    }
}

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
            // Retained in the wire request for compatibility, but the daemon
            // owns one fixed coding-agent system prompt.
            system_prompt: None,
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
            system_prompt: None,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDefinitionDto {
    #[serde(default)]
    pub mode: PromptMode,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamePolicyDto {
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutAgentProfileRequest {
    #[serde(default)]
    pub prompt: PromptDefinitionDto,
    #[serde(default)]
    pub tools: NamePolicyDto,
    #[serde(default)]
    pub skills: NamePolicyDto,
    #[serde(default, rename = "initial_agent_mode")]
    _initial_agent_mode: Option<LegacyAgentMode>,
    #[serde(default)]
    pub initial_capability_mode: CapabilityMode,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyAgentMode {
    Default,
    Plan,
}

impl From<PutAgentProfileRequest> for AgentProfileDefinition {
    fn from(request: PutAgentProfileRequest) -> Self {
        Self {
            prompt: PromptDefinition {
                mode: request.prompt.mode,
                text: request.prompt.text,
            },
            tools: NamePolicy {
                allow: request.tools.allow,
                deny: request.tools.deny,
            },
            skills: NamePolicy {
                allow: request.skills.allow,
                deny: request.skills.deny,
            },
            initial_capability_mode: request.initial_capability_mode,
            model: request.model,
            reasoning_effort: request.reasoning_effort,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicAgentProfile {
    pub agent_profile_id: String,
    pub revision: u64,
    pub prompt: PromptDefinitionDto,
    pub tools: NamePolicyDto,
    pub skills: NamePolicyDto,
    pub initial_capability_mode: CapabilityMode,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl From<AgentProfile> for PublicAgentProfile {
    fn from(profile: AgentProfile) -> Self {
        let definition = profile.definition;
        Self {
            agent_profile_id: profile.agent_profile_id,
            revision: profile.revision,
            prompt: PromptDefinitionDto {
                mode: definition.prompt.mode,
                text: definition.prompt.text,
            },
            tools: NamePolicyDto {
                allow: definition.tools.allow,
                deny: definition.tools.deny,
            },
            skills: NamePolicyDto {
                allow: definition.skills.allow,
                deny: definition.skills.deny,
            },
            initial_capability_mode: definition.initial_capability_mode,
            model: definition.model,
            reasoning_effort: definition.reasoning_effort,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AgentProfileResponse {
    pub configured: bool,
    pub agent_profile: Option<PublicAgentProfile>,
}

impl AgentProfileResponse {
    pub fn from_profile(profile: Option<AgentProfile>) -> Self {
        Self {
            configured: profile.is_some(),
            agent_profile: profile.map(PublicAgentProfile::from),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AgentProfilesResponse {
    pub agent_profiles: Vec<PublicAgentProfile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentProfileRefDto {
    pub agent_profile_id: String,
    pub revision: u64,
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
    Compact {
        request_id: String,
        #[serde(default)]
        instructions: Option<String>,
    },
    SetModel {
        request_id: String,
        model: String,
    },
    SetReasoningEffort {
        request_id: String,
        effort: Option<ReasoningEffort>,
    },
    SetCapabilityMode {
        request_id: String,
        capability_mode: CapabilityMode,
    },
    #[serde(rename = "answer_askuser")]
    AnswerAskUser {
        request_id: String,
        ask_id: AskUserId,
        answers: Vec<AskUserAnswer>,
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
            | Self::Compact { request_id, .. }
            | Self::SetModel { request_id, .. }
            | Self::SetReasoningEffort { request_id, .. }
            | Self::SetCapabilityMode { request_id, .. }
            | Self::AnswerAskUser { request_id, .. }
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
        capability_mode: CapabilityMode,
        agent_profile: AgentProfileRefDto,
        workspace: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills: Vec<SkillSummaryDto>,
    },
    SessionCreated {
        session_id: SessionId,
    },
    Snapshot {
        session: SessionDto,
    },
    /// Initial state for the read-only child-agent observer endpoint.
    SubagentSnapshot {
        subagent: SubagentSnapshotDto,
        input_allowed: bool,
    },
    /// One ordered event from a child-agent observer stream.
    SubagentEvent {
        sequence: u64,
        parent_session_id: String,
        agent_id: String,
        event: SubagentEventDto,
    },
    /// The observer fell behind and must replace its local child state.
    SubagentResyncRequired {
        skipped: u64,
        subagent: SubagentSnapshotDto,
        input_allowed: bool,
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
pub struct SubagentSnapshotDto {
    pub parent_session_id: String,
    pub agent_id: String,
    pub description: String,
    pub effective_config: EffectiveSubagentConfig,
    pub state: SubagentState,
    pub active_run: Option<ActiveSubagentRun>,
    pub messages: Vec<PublicMessage>,
    pub draft: Option<String>,
    pub cumulative_usage: TokenUsage,
    pub context_usage: Option<ContextUsageDto>,
    pub last_outcome: Option<SubagentRunOutcome>,
    pub validated_output: Option<ValidatedSubagentOutput>,
    pub resource: Option<SubagentResourceInfo>,
    pub resource_finalization: Option<SubagentResourceFinalization>,
    pub last_sequence: u64,
}

impl From<&SubagentSnapshot> for SubagentSnapshotDto {
    fn from(snapshot: &SubagentSnapshot) -> Self {
        Self {
            parent_session_id: snapshot.parent_id.clone(),
            agent_id: snapshot.agent_id.clone(),
            description: snapshot.description.clone(),
            effective_config: snapshot.effective_config.clone(),
            state: snapshot.state.clone(),
            active_run: snapshot.active_run.clone(),
            messages: snapshot.messages.iter().map(PublicMessage::from).collect(),
            draft: snapshot.draft.clone(),
            cumulative_usage: snapshot.cumulative_usage,
            context_usage: snapshot.context_usage.map(ContextUsageDto::from),
            last_outcome: snapshot.last_outcome.clone(),
            validated_output: snapshot.validated_output.clone(),
            resource: snapshot.resource.clone(),
            resource_finalization: snapshot.resource_finalization.clone(),
            last_sequence: snapshot.last_sequence,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubagentEventDto {
    Spawned {
        description: String,
        initial_delivery_id: String,
        effective_config: EffectiveSubagentConfig,
    },
    StateChanged {
        state: SubagentState,
    },
    MessageQueued {
        delivery_id: String,
    },
    Notification {
        notification: SubagentNotification,
    },
    AgentEvent {
        event: Box<EventDto>,
    },
    RunFinished {
        run_id: String,
        outcome: SubagentRunOutcome,
    },
    OutputValidated {
        output: ValidatedSubagentOutput,
    },
    ResourceFinalized {
        finalization: SubagentResourceFinalization,
    },
    ResourceFinalizationFailed {
        error: String,
    },
    Closed {
        delivery_id: String,
        reason: String,
        wake_parent: bool,
    },
}

impl From<SubagentEventKind> for SubagentEventDto {
    fn from(event: SubagentEventKind) -> Self {
        match event {
            SubagentEventKind::Spawned {
                description,
                initial_delivery_id,
                effective_config,
            } => Self::Spawned {
                description,
                initial_delivery_id,
                effective_config,
            },
            SubagentEventKind::StateChanged { state } => Self::StateChanged { state },
            SubagentEventKind::MessageQueued { delivery_id } => Self::MessageQueued { delivery_id },
            SubagentEventKind::Notification(notification) => Self::Notification { notification },
            SubagentEventKind::AgentEvent(event) => Self::AgentEvent {
                event: Box::new(EventDto::from(event)),
            },
            SubagentEventKind::RunFinished { run_id, outcome } => {
                Self::RunFinished { run_id, outcome }
            }
            SubagentEventKind::OutputValidated { output } => Self::OutputValidated { output },
            SubagentEventKind::ResourceFinalized { finalization } => {
                Self::ResourceFinalized { finalization }
            }
            SubagentEventKind::ResourceFinalizationFailed { error } => {
                Self::ResourceFinalizationFailed { error }
            }
            SubagentEventKind::Closed {
                delivery_id,
                reason,
                wake_parent,
            } => Self::Closed {
                delivery_id,
                reason,
                wake_parent,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionDto {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub profile_id: String,
    pub agent_profile: AgentProfileRefDto,
    pub workspace: Option<String>,
    pub initialized: bool,
    pub status: SessionStatusDto,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub capability_mode: CapabilityMode,
    pub config: SessionConfigDto,
    pub history: Vec<PublicMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_compactions: Vec<ContextCompactionStatusDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_compaction: Option<ContextCompactionStatusDto>,
    pub draft: Option<AssistantDraftDto>,
    pub pending_asks: Vec<AskUserRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillSummaryDto>,
    pub subagents: Vec<SubagentSummaryDto>,
    pub usage: UsageDto,
    pub last_sequence: u64,
}

impl From<&AgentView> for SessionDto {
    fn from(view: &AgentView) -> Self {
        Self {
            session_id: view.session_id,
            title: view.title.clone(),
            profile_id: view.profile_id.clone(),
            agent_profile: AgentProfileRefDto {
                agent_profile_id: view.agent_profile_id.clone(),
                revision: view.agent_profile_revision,
            },
            workspace: workspace_path(view.workspace.as_ref()),
            initialized: view.initialized,
            status: view.status.into(),
            active_run_id: view.active_run_id,
            queued_runs: view.queued_runs,
            capability_mode: view.capability_mode,
            config: SessionConfigDto {
                model: view.model.clone(),
                reasoning_effort: view.reasoning_effort,
                revision: view.config_revision,
            },
            history: public_history(view),
            context_compactions: public_compactions(view),
            context_compaction: view
                .context_compaction
                .as_ref()
                .map(ContextCompactionStatusDto::from),
            draft: view.draft.as_ref().map(AssistantDraftDto::from),
            pending_asks: view.pending_asks.clone(),
            skills: Vec::new(),
            subagents: view
                .subagents
                .iter()
                .map(|summary| SubagentSummaryDto::from_parent(summary, view.session_id))
                .collect(),
            usage: UsageDto {
                last: view.last_usage,
                context: view.context_usage.map(ContextUsageDto::from),
                cumulative: view.cumulative_usage,
            },
            last_sequence: view.last_event_sequence,
        }
    }
}

impl SessionDto {
    pub fn from_view_with_skills(view: &AgentView, skills: &[SkillMetadata]) -> Self {
        let mut session = Self::from(view);
        session.skills = skills.iter().map(SkillSummaryDto::from).collect();
        session
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SubagentSummaryDto {
    pub agent_id: String,
    pub description: String,
    pub state: SubagentState,
    pub last_sequence: u64,
    pub observer_path: String,
}

impl SubagentSummaryDto {
    fn from_parent(summary: &SubagentSummary, parent_session_id: impl fmt::Display) -> Self {
        Self {
            agent_id: summary.agent_id.clone(),
            description: summary.description.clone(),
            state: summary.state.clone(),
            last_sequence: summary.last_sequence,
            observer_path: format!(
                "/v1/ws/attach/{parent_session_id}/subagents/{}",
                summary.agent_id
            ),
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
    Compacting,
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
            AgentStatus::Compacting => Self::Compacting,
            AgentStatus::Running => Self::Running,
            AgentStatus::Stopping => Self::Stopping,
            AgentStatus::Closing => Self::Closing,
            AgentStatus::Closed => Self::Closed,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionPhaseDto {
    Started,
    Completed,
    Failed,
}

impl From<ContextCompactionPhase> for ContextCompactionPhaseDto {
    fn from(value: ContextCompactionPhase) -> Self {
        match value {
            ContextCompactionPhase::Started => Self::Started,
            ContextCompactionPhase::Completed => Self::Completed,
            ContextCompactionPhase::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextCompactionStatusDto {
    pub phase: ContextCompactionPhaseDto,
    pub history_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_message_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl From<&ContextCompactionView> for ContextCompactionStatusDto {
    fn from(value: &ContextCompactionView) -> Self {
        Self {
            phase: value.phase.into(),
            history_index: value.history_index,
            after_message_count: value.after_message_count,
            message: value.message.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicMessage {
    pub role: Role,
    #[serde(default, skip_serializing_if = "MessageVisibility::is_public")]
    pub visibility: MessageVisibility,
    pub content: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub tool_result_is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_metadata: Option<Value>,
}

impl PublicMessage {
    fn redacted(message: &Message) -> Self {
        Self {
            role: message.role.clone(),
            visibility: MessageVisibility::Internal,
            content: None,
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_result_is_error: false,
            tool_result_metadata: None,
        }
    }
}

impl From<&Message> for PublicMessage {
    fn from(message: &Message) -> Self {
        let visibility = public_message_visibility(message);
        if visibility == MessageVisibility::Internal {
            return Self::redacted(message);
        }
        Self {
            role: message.role.clone(),
            visibility,
            content: message.content.clone(),
            reasoning: message
                .reasoning
                .clone()
                .filter(|reasoning| !reasoning.is_empty())
                .or_else(|| {
                    message
                        .provider_state
                        .as_ref()
                        .and_then(ProviderState::reasoning_text)
                }),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            tool_result_is_error: message.tool_result_is_error,
            tool_result_metadata: message.tool_result_metadata.clone(),
        }
    }
}

fn public_history(view: &AgentView) -> Vec<PublicMessage> {
    view.display_messages
        .iter()
        .map(PublicMessage::from)
        .collect()
}

fn public_compactions(view: &AgentView) -> Vec<ContextCompactionStatusDto> {
    let mut compactions = view
        .context_compactions
        .iter()
        .map(ContextCompactionStatusDto::from)
        .collect::<Vec<_>>();
    if let Some(current) = view.context_compaction.as_ref()
        && view.context_compactions.last() != Some(current)
    {
        compactions.push(ContextCompactionStatusDto::from(current));
    }
    compactions
}

fn public_message_visibility(message: &Message) -> MessageVisibility {
    if message.visibility == MessageVisibility::Internal || is_legacy_subagent_notification(message)
    {
        MessageVisibility::Internal
    } else {
        MessageVisibility::Public
    }
}

fn is_legacy_subagent_notification(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }
    let Some(payload) = message
        .text_content()
        .and_then(|text| text.strip_prefix("<subagent_notification>"))
        .and_then(|text| text.strip_suffix("</subagent_notification>"))
    else {
        return false;
    };
    let Ok(payload) = serde_json::from_str::<Value>(payload) else {
        return false;
    };
    payload["type"] == "subagent_notification"
        && payload["agent_id"].is_string()
        && payload["sequence"].is_u64()
        && payload["delivery_id"].is_string()
        && payload["kind"].is_string()
        && payload["source"].is_string()
        && payload["message"].is_string()
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantDraftDto {
    pub reasoning: String,
    pub text: String,
    pub tool_calls: Vec<ToolCallDraftDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_message_index: Option<usize>,
}

impl From<&AssistantDraft> for AssistantDraftDto {
    fn from(draft: &AssistantDraft) -> Self {
        Self {
            reasoning: draft.reasoning.clone(),
            text: draft.text.clone(),
            tool_calls: draft
                .tool_calls
                .iter()
                .map(ToolCallDraftDto::from)
                .collect(),
            fork_message_index: draft.fork_message_index,
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
    pub workspaces: Vec<WorkspaceSessionsDto>,
}

impl SessionsResponse {
    pub fn from_sessions(sessions: Vec<SessionSummaryDto>) -> Self {
        let mut workspaces: Vec<WorkspaceSessionsDto> = Vec::new();
        let mut workspace_indices = HashMap::<Option<String>, usize>::new();
        for session in &sessions {
            let workspace = session.workspace.clone();
            let index = match workspace_indices.entry(workspace.clone()) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let index = workspaces.len();
                    entry.insert(index);
                    workspaces.push(WorkspaceSessionsDto {
                        workspace,
                        sessions: Vec::new(),
                    });
                    index
                }
            };
            workspaces[index].sessions.push(session.clone());
        }
        Self {
            sessions,
            workspaces,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorkspaceSessionsDto {
    pub workspace: Option<String>,
    pub sessions: Vec<SessionSummaryDto>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionRequest {
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkSessionRequest {
    pub message_index: usize,
    #[serde(default)]
    pub position: ForkPositionDto,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ForkPositionDto {
    #[default]
    After,
    BeforeToolCalls,
}

impl From<ForkPositionDto> for ForkPosition {
    fn from(position: ForkPositionDto) -> Self {
        match position {
            ForkPositionDto::After => Self::After,
            ForkPositionDto::BeforeToolCalls => Self::BeforeToolCalls,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SkillsResponse {
    pub session_id: SessionId,
    pub skills: Vec<SkillSummaryDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<SkillDiagnosticDto>,
}

#[derive(Clone, Debug, Serialize)]
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
pub struct WorkspaceDirectoryDto {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceBrowseResponse {
    pub path: String,
    pub parent: Option<String>,
    pub directories: Vec<WorkspaceDirectoryDto>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummaryDto {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub pinned: bool,
    pub profile_id: String,
    pub agent_profile: AgentProfileRefDto,
    pub workspace: Option<String>,
    pub status: SessionStatusDto,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub capability_mode: Option<CapabilityMode>,
    pub config: SessionConfigDto,
    pub message_count: Option<usize>,
    pub subagents: Vec<SubagentSummaryDto>,
}

impl From<SessionListing> for SessionSummaryDto {
    fn from(listing: SessionListing) -> Self {
        let pinned = listing.record.pinned;
        match listing.state {
            Some(state) => Self {
                session_id: state.session_id,
                title: state.title,
                pinned,
                profile_id: state.profile_id,
                agent_profile: AgentProfileRefDto {
                    agent_profile_id: state.agent_profile_id,
                    revision: state.agent_profile_revision,
                },
                workspace: workspace_path(state.workspace.as_ref()),
                status: state.status.into(),
                active_run_id: state.active_run_id,
                queued_runs: state.queued_runs,
                capability_mode: Some(state.capability_mode),
                config: SessionConfigDto {
                    model: state.model,
                    reasoning_effort: state.reasoning_effort,
                    revision: state.config_revision,
                },
                message_count: Some(state.message_count),
                subagents: state
                    .subagents
                    .iter()
                    .map(|summary| SubagentSummaryDto::from_parent(summary, state.session_id))
                    .collect(),
            },
            None => Self {
                session_id: listing.record.id,
                title: listing.record.title,
                pinned,
                profile_id: listing.record.profile_id,
                agent_profile: listing
                    .record
                    .agent_profile
                    .as_ref()
                    .map(|profile| AgentProfileRefDto {
                        agent_profile_id: profile.agent_profile_id.clone(),
                        revision: profile.revision,
                    })
                    .unwrap_or_else(|| AgentProfileRefDto {
                        agent_profile_id: crate::runtime::DEFAULT_AGENT_PROFILE_ID.to_owned(),
                        revision: crate::runtime::DEFAULT_AGENT_PROFILE_REVISION,
                    }),
                workspace: workspace_path(listing.record.workspace.as_ref()),
                status: SessionStatusDto::Offline,
                active_run_id: None,
                queued_runs: 0,
                capability_mode: None,
                config: SessionConfigDto {
                    model: listing.record.model,
                    reasoning_effort: listing.record.reasoning_effort,
                    revision: listing.record.config_revision,
                },
                message_count: None,
                subagents: Vec::new(),
            },
        }
    }
}

fn workspace_path(workspace: Option<&phi::Workspace>) -> Option<String> {
    workspace.map(ToString::to_string)
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventDto {
    StateChanged {
        status: SessionStatusDto,
    },
    SessionInitialized,
    TitleChanged {
        title: String,
    },
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
    CapabilityModeChanged {
        capability_mode: CapabilityMode,
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
    OperationFailed {
        operation: String,
        message: String,
    },
    ActorCrashed {
        message: String,
    },
    SubagentSpawned {
        agent_id: String,
        description: String,
        initial_delivery_id: String,
        effective_config: EffectiveSubagentConfig,
        observer_path: String,
    },
    SubagentStateChanged {
        agent_id: String,
        state: SubagentState,
    },
    SubagentMessageQueued {
        agent_id: String,
        delivery_id: String,
    },
    SubagentNotification {
        agent_id: String,
        notification: SubagentNotification,
    },
    SubagentAgentEvent {
        agent_id: String,
        event: Box<EventDto>,
    },
    SubagentRunFinished {
        agent_id: String,
        run_id: String,
        outcome: SubagentRunOutcome,
    },
    SubagentOutputValidated {
        agent_id: String,
        output: ValidatedSubagentOutput,
    },
    SubagentResourceFinalized {
        agent_id: String,
        finalization: SubagentResourceFinalization,
    },
    SubagentResourceFinalizationFailed {
        agent_id: String,
        error: String,
    },
    SubagentClosed {
        agent_id: String,
        delivery_id: String,
        reason: String,
        wake_parent: bool,
    },
    SubagentsResynced {
        subagents: Vec<SubagentSummaryDto>,
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
    ContextCompactionStarted {
        trigger: ContextCompactionTriggerDto,
        compactor: String,
    },
    ContextCompactionCompleted {
        trigger: ContextCompactionTriggerDto,
        compactor: String,
        before_message_count: usize,
        after_message_count: usize,
        usage: Option<TokenUsage>,
        estimated_context_tokens: u64,
    },
    ContextCompactionFailed {
        trigger: ContextCompactionTriggerDto,
        compactor: String,
        message: String,
    },
    Error {
        message: String,
    },
}

impl From<RuntimeEvent> for ServerMessage {
    fn from(event: RuntimeEvent) -> Self {
        let session_id = event.session_id;
        Self::Event {
            sequence: event.sequence,
            session_id,
            run_id: event.run_id,
            event: EventDto::from_runtime(event.kind, session_id),
        }
    }
}

impl EventDto {
    fn from_runtime(event: RuntimeEventKind, parent_session_id: SessionId) -> Self {
        match event {
            RuntimeEventKind::StateChanged { status } => Self::StateChanged {
                status: status.into(),
            },
            RuntimeEventKind::SessionInitialized => Self::SessionInitialized,
            RuntimeEventKind::TitleChanged { title } => Self::TitleChanged { title },
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
            RuntimeEventKind::CapabilityModeChanged { capability_mode } => {
                Self::CapabilityModeChanged { capability_mode }
            }
            RuntimeEventKind::AskUserRequested { request } => Self::AskUserRequested { request },
            RuntimeEventKind::AskUserAnswered { ask_id } => Self::AskUserAnswered { ask_id },
            RuntimeEventKind::AskUserCancelled { ask_id } => Self::AskUserCancelled { ask_id },
            RuntimeEventKind::OperationFailed { operation, message } => {
                Self::OperationFailed { operation, message }
            }
            RuntimeEventKind::ActorCrashed { message } => Self::ActorCrashed { message },
            RuntimeEventKind::Subagent(event) => Self::from(event),
            RuntimeEventKind::SubagentsResynced { subagents } => Self::SubagentsResynced {
                subagents: subagents
                    .iter()
                    .map(|summary| SubagentSummaryDto::from_parent(summary, parent_session_id))
                    .collect(),
            },
            RuntimeEventKind::Agent(event) => Self::from(event),
        }
    }
}

impl From<SubagentEvent> for EventDto {
    fn from(event: SubagentEvent) -> Self {
        let agent_id = event.agent_id;
        match event.kind {
            SubagentEventKind::Spawned {
                description,
                initial_delivery_id,
                effective_config,
            } => Self::SubagentSpawned {
                observer_path: format!("/v1/ws/attach/{}/subagents/{agent_id}", event.parent_id),
                agent_id,
                description,
                initial_delivery_id,
                effective_config,
            },
            SubagentEventKind::StateChanged { state } => {
                Self::SubagentStateChanged { agent_id, state }
            }
            SubagentEventKind::MessageQueued { delivery_id } => Self::SubagentMessageQueued {
                agent_id,
                delivery_id,
            },
            SubagentEventKind::Notification(notification) => Self::SubagentNotification {
                agent_id,
                notification,
            },
            SubagentEventKind::AgentEvent(event) => Self::SubagentAgentEvent {
                agent_id,
                event: Box::new(Self::from(event)),
            },
            SubagentEventKind::RunFinished { run_id, outcome } => Self::SubagentRunFinished {
                agent_id,
                run_id,
                outcome,
            },
            SubagentEventKind::OutputValidated { output } => {
                Self::SubagentOutputValidated { agent_id, output }
            }
            SubagentEventKind::ResourceFinalized { finalization } => {
                Self::SubagentResourceFinalized {
                    agent_id,
                    finalization,
                }
            }
            SubagentEventKind::ResourceFinalizationFailed { error } => {
                Self::SubagentResourceFinalizationFailed { agent_id, error }
            }
            SubagentEventKind::Closed {
                delivery_id,
                reason,
                wake_parent,
            } => Self::SubagentClosed {
                agent_id,
                delivery_id,
                reason,
                wake_parent,
            },
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
                message: PublicMessage::from(message.as_ref()),
                tool_results: tool_results.iter().map(PublicMessage::from).collect(),
            },
            AgentEvent::MessageStart { message } => Self::MessageStart {
                message: PublicMessage::from(message.as_ref()),
            },
            AgentEvent::MessageUpdate { delta } => Self::MessageUpdate {
                delta: delta.into(),
            },
            AgentEvent::MessageEnd { message } => Self::MessageEnd {
                message: PublicMessage::from(message.as_ref()),
            },
            AgentEvent::MessageAborted => Self::MessageAborted,
            AgentEvent::ToolExecutionStart { call } => Self::ToolExecutionStart {
                call: call.as_ref().clone(),
            },
            AgentEvent::ToolExecutionProgress { call, progress } => Self::ToolExecutionProgress {
                call: call.as_ref().clone(),
                progress,
            },
            AgentEvent::ToolExecutionEnd {
                call,
                content,
                is_error,
                content_parts,
                metadata,
            } => Self::ToolExecutionEnd {
                call: call.as_ref().clone(),
                content,
                is_error,
                content_parts: content_parts.to_vec(),
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
            AgentEvent::ContextCompactionStarted {
                trigger, compactor, ..
            } => Self::ContextCompactionStarted {
                trigger: trigger.into(),
                compactor,
            },
            AgentEvent::ContextCompactionCompleted {
                trigger,
                compactor,
                before_message_count,
                after_message_count,
                usage,
                estimated_context_tokens,
                ..
            } => Self::ContextCompactionCompleted {
                trigger: trigger.into(),
                compactor,
                before_message_count,
                after_message_count,
                usage,
                estimated_context_tokens,
            },
            AgentEvent::ContextCompactionFailed {
                trigger,
                compactor,
                message,
            } => Self::ContextCompactionFailed {
                trigger: trigger.into(),
                compactor,
                message,
            },
            AgentEvent::Error { message } => Self::Error { message },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextCompactionTriggerDto {
    Automatic { usage: ContextUsageDto },
    Manual { instructions: Option<String> },
    ContextLengthExceeded,
}

impl From<ContextCompactionTrigger> for ContextCompactionTriggerDto {
    fn from(trigger: ContextCompactionTrigger) -> Self {
        match trigger {
            ContextCompactionTrigger::Automatic { usage } => Self::Automatic {
                usage: usage.into(),
            },
            ContextCompactionTrigger::Manual { instructions } => Self::Manual { instructions },
            ContextCompactionTrigger::ContextLengthExceeded { .. } => Self::ContextLengthExceeded,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeltaDto {
    Reasoning {
        delta: String,
    },
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
            AssistantDelta::Reasoning { delta } => Self::Reasoning { delta },
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
    fn scheduled_task_request_debug_redacts_prompt() {
        let secret = "canary-scheduled-prompt-that-must-not-appear-in-debug";
        let request: CreateScheduledTaskRequest = serde_json::from_value(serde_json::json!({
            "name": "review",
            "prompt": secret,
            "schedule": {
                "type": "interval",
                "every": 1,
                "unit": "hours"
            }
        }))
        .unwrap();

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
    fn provider_system_prompt_is_accepted_for_compatibility_but_ignored() {
        let request: PutProviderRequest = serde_json::from_value(serde_json::json!({
            "provider": "openai_chat",
            "api_key": "secret",
            "base_url": "https://example.test/v1",
            "model": "test-model",
            "system_prompt": "custom prompt",
            "max_context_tokens": 128000
        }))
        .unwrap();
        let config = ProviderConfig::from(request);
        assert_eq!(config.system_prompt, None);

        let mut legacy = config;
        legacy.system_prompt = Some("legacy prompt".to_owned());
        let public = PublicProviderConfig::new(crate::store::DEFAULT_PROFILE_ID, &legacy);
        assert_eq!(public.system_prompt, None);
    }

    #[test]
    fn agent_profile_request_defaults_and_rejects_unknown_fields() {
        let defaults: PutAgentProfileRequest =
            serde_json::from_value(serde_json::json!({})).unwrap();
        let defaults = AgentProfileDefinition::from(defaults);
        assert_eq!(defaults.prompt.mode, PromptMode::Extend);
        assert_eq!(defaults.prompt.text, "");
        assert_eq!(defaults.tools, NamePolicy::default());
        assert_eq!(defaults.skills, NamePolicy::default());
        assert_eq!(defaults.initial_capability_mode, CapabilityMode::FullAccess);
        assert_eq!(defaults.model, None);
        assert_eq!(defaults.reasoning_effort, None);

        let configured: PutAgentProfileRequest = serde_json::from_value(serde_json::json!({
            "prompt": {
                "mode": "full",
                "text": "Act as a focused reviewer."
            },
            "tools": {
                "allow": ["read", "edit"],
                "deny": ["bash"]
            },
            "skills": {
                "allow": ["rust-review"],
                "deny": []
            },
            "initial_agent_mode": "plan",
            "initial_capability_mode": "workspace_edit",
            "model": "review-model",
            "reasoning_effort": "high"
        }))
        .unwrap();
        let configured = AgentProfileDefinition::from(configured);
        assert_eq!(configured.prompt.mode, PromptMode::Full);
        assert_eq!(
            configured.tools.allow,
            Some(vec!["read".to_owned(), "edit".to_owned()])
        );
        assert_eq!(configured.tools.deny, vec!["bash"]);
        assert_eq!(
            configured.skills.allow,
            Some(vec!["rust-review".to_owned()])
        );
        assert_eq!(
            configured.initial_capability_mode,
            CapabilityMode::WorkspaceEdit
        );
        assert_eq!(configured.model.as_deref(), Some("review-model"));
        assert_eq!(configured.reasoning_effort, Some(ReasoningEffort::High));

        assert!(
            serde_json::from_value::<PutAgentProfileRequest>(serde_json::json!({
                "unknown": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<PutAgentProfileRequest>(serde_json::json!({
                "prompt": {
                    "mode": "extend",
                    "text": "valid",
                    "unknown": true
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn agent_profile_response_serializes_the_public_definition() {
        let profile = AgentProfile {
            agent_profile_id: "reviewer".to_owned(),
            revision: 3,
            definition: AgentProfileDefinition {
                prompt: PromptDefinition {
                    mode: PromptMode::Extend,
                    text: "Review before editing.".to_owned(),
                },
                tools: NamePolicy {
                    allow: Some(vec!["read".to_owned(), "edit".to_owned()]),
                    deny: vec!["bash".to_owned()],
                },
                skills: NamePolicy {
                    allow: Some(vec!["rust-review".to_owned()]),
                    deny: Vec::new(),
                },
                initial_capability_mode: CapabilityMode::WorkspaceEdit,
                model: Some("review-model".to_owned()),
                reasoning_effort: Some(ReasoningEffort::High),
            },
        };

        let value =
            serde_json::to_value(AgentProfileResponse::from_profile(Some(profile))).unwrap();
        assert_eq!(value["configured"], true);
        assert_eq!(value["agent_profile"]["agent_profile_id"], "reviewer");
        assert_eq!(value["agent_profile"]["revision"], 3);
        assert_eq!(value["agent_profile"]["prompt"]["mode"], "extend");
        assert!(value["agent_profile"].get("initial_agent_mode").is_none());
        assert_eq!(
            value["agent_profile"]["initial_capability_mode"],
            "workspace_edit"
        );
        assert_eq!(value["agent_profile"]["tools"]["allow"][0], "read");
        assert_eq!(value["agent_profile"]["skills"]["allow"][0], "rust-review");
        assert_eq!(value["agent_profile"]["model"], "review-model");
        assert_eq!(value["agent_profile"]["reasoning_effort"], "high");
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

    #[test]
    fn ready_message_exposes_the_selected_workspace() {
        let message = ServerMessage::Ready {
            config: SessionConfigDto {
                model: "test-model".to_owned(),
                reasoning_effort: None,
                revision: 0,
            },
            capability_mode: CapabilityMode::FullAccess,
            agent_profile: AgentProfileRefDto {
                agent_profile_id: crate::runtime::DEFAULT_AGENT_PROFILE_ID.to_owned(),
                revision: crate::runtime::DEFAULT_AGENT_PROFILE_REVISION,
            },
            workspace: Some("/workspace/project".to_owned()),
            skills: vec![SkillSummaryDto {
                name: "review".to_owned(),
                display_name: Some("Code review".to_owned()),
                description: "Review the current change".to_owned(),
                when_to_use: None,
                argument_hint: Some("[focus]".to_owned()),
                arguments: Vec::new(),
                version: None,
                model_invocable: true,
                user_invocable: true,
                source: Some("workspace".to_owned()),
            }],
        };
        let value = serde_json::to_value(message).unwrap();

        assert_eq!(value["type"], "ready");
        assert_eq!(value["workspace"], "/workspace/project");
        assert_eq!(value["capability_mode"], "full_access");
        assert_eq!(value["skills"][0]["name"], "review");
        assert_eq!(value["skills"][0]["user_invocable"], true);
        assert!(value.get("mode").is_none());
        assert_eq!(
            value["agent_profile"]["agent_profile_id"],
            crate::runtime::DEFAULT_AGENT_PROFILE_ID
        );
        assert_eq!(
            value["agent_profile"]["revision"],
            crate::runtime::DEFAULT_AGENT_PROFILE_REVISION
        );
    }

    #[test]
    fn removed_plan_mode_commands_are_not_part_of_the_wire_protocol() {
        assert!(
            serde_json::from_value::<ClientCommand>(serde_json::json!({
                "type": "set_mode",
                "request_id": "legacy-mode",
                "mode": "plan"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ClientCommand>(serde_json::json!({
                "type": "decide_plan_approval",
                "request_id": "legacy-approval",
                "approval_id": "019c0000-0000-7000-8000-000000000001",
                "decision": {
                    "type": "approve",
                    "revision": 1
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn fork_request_defaults_after_and_accepts_before_tool_calls() {
        let legacy: ForkSessionRequest = serde_json::from_value(serde_json::json!({
            "message_index": 7
        }))
        .unwrap();
        assert_eq!(legacy.message_index, 7);
        assert_eq!(legacy.position, ForkPositionDto::After);

        let intermediate: ForkSessionRequest = serde_json::from_value(serde_json::json!({
            "message_index": 9,
            "position": "before_tool_calls"
        }))
        .unwrap();
        assert_eq!(intermediate.message_index, 9);
        assert_eq!(intermediate.position, ForkPositionDto::BeforeToolCalls);

        assert!(
            serde_json::from_value::<ForkSessionRequest>(serde_json::json!({
                "message_index": 9,
                "position": "during_tool_call"
            }))
            .is_err()
        );
    }

    #[test]
    fn capability_command_and_event_use_stable_wire_names() {
        let command: ClientCommand = serde_json::from_value(serde_json::json!({
            "type": "set_capability_mode",
            "request_id": "capability-1",
            "capability_mode": "read_only"
        }))
        .unwrap();
        assert!(matches!(
            command,
            ClientCommand::SetCapabilityMode {
                request_id,
                capability_mode: CapabilityMode::ReadOnly,
            } if request_id == "capability-1"
        ));

        let event = serde_json::to_value(EventDto::CapabilityModeChanged {
            capability_mode: CapabilityMode::FullAccess,
        })
        .unwrap();
        assert_eq!(
            event,
            serde_json::json!({
                "type": "capability_mode_changed",
                "capability_mode": "full_access"
            })
        );
    }

    #[test]
    fn generated_title_uses_a_stable_wire_event_and_summary_field() {
        let event = serde_json::to_value(EventDto::TitleChanged {
            title: "Fix flaky storage tests".to_owned(),
        })
        .unwrap();
        assert_eq!(
            event,
            serde_json::json!({
                "type": "title_changed",
                "title": "Fix flaky storage tests"
            })
        );

        let summary = SessionSummaryDto {
            session_id: SessionId::new(),
            title: Some("Fix flaky storage tests".to_owned()),
            pinned: true,
            profile_id: "default".to_owned(),
            agent_profile: AgentProfileRefDto {
                agent_profile_id: crate::runtime::DEFAULT_AGENT_PROFILE_ID.to_owned(),
                revision: crate::runtime::DEFAULT_AGENT_PROFILE_REVISION,
            },
            workspace: None,
            status: SessionStatusDto::Idle,
            active_run_id: None,
            queued_runs: 0,
            capability_mode: Some(CapabilityMode::FullAccess),
            config: SessionConfigDto {
                model: "test-model".to_owned(),
                reasoning_effort: None,
                revision: 0,
            },
            message_count: Some(2),
            subagents: Vec::new(),
        };
        let summary = serde_json::to_value(summary).unwrap();
        assert_eq!(summary["title"], "Fix flaky storage tests");
        assert_eq!(summary["pinned"], true);
    }

    #[test]
    fn reasoning_uses_normalized_public_fields_and_stable_delta_shape() {
        let mut message = Message::assistant(Some(Content::text("done")), Vec::new());
        message.provider_state = Some(ProviderState::AnthropicMessages {
            content: vec![serde_json::json!({
                "type": "thinking",
                "thinking": "inspect inputs",
                "signature": "opaque-signature"
            })],
        });

        let public = serde_json::to_value(PublicMessage::from(&message)).unwrap();
        assert_eq!(public["reasoning"], "inspect inputs");
        assert!(public.get("provider_state").is_none());
        assert!(!public.to_string().contains("opaque-signature"));

        let delta = serde_json::to_value(DeltaDto::from(AssistantDelta::Reasoning {
            delta: "inspect".to_owned(),
        }))
        .unwrap();
        assert_eq!(
            delta,
            serde_json::json!({
                "type": "reasoning",
                "delta": "inspect"
            })
        );
    }

    #[test]
    fn public_messages_mark_internal_and_upgrade_legacy_subagent_wakes() {
        let public = serde_json::to_value(PublicMessage::from(&Message::user("hello"))).unwrap();
        assert!(public.get("visibility").is_none());

        let internal =
            Message::user("runtime coordination").with_visibility(MessageVisibility::Internal);
        let internal = serde_json::to_value(PublicMessage::from(&internal)).unwrap();
        assert_eq!(internal["visibility"], "internal");
        assert_eq!(internal["content"], Value::Null);
        assert!(!internal.to_string().contains("runtime coordination"));

        let legacy = Message::user(format!(
            "<subagent_notification>{}</subagent_notification>",
            serde_json::json!({
                "type": "subagent_notification",
                "agent_id": "agent-1",
                "sequence": 7,
                "delivery_id": "delivery-1",
                "kind": "result",
                "source": "runtime",
                "message": "done"
            })
        ));
        let legacy = serde_json::to_value(PublicMessage::from(&legacy)).unwrap();
        assert_eq!(legacy["visibility"], "internal");

        let user_authored =
            Message::user("<subagent_notification>not valid JSON</subagent_notification>");
        let user_authored = serde_json::to_value(PublicMessage::from(&user_authored)).unwrap();
        assert!(user_authored.get("visibility").is_none());
    }

    #[test]
    fn compact_command_accepts_optional_instructions() {
        let plain: ClientCommand = serde_json::from_value(serde_json::json!({
            "type": "compact",
            "request_id": "compact-plain"
        }))
        .unwrap();
        assert!(matches!(
            plain,
            ClientCommand::Compact {
                request_id,
                instructions: None,
            } if request_id == "compact-plain"
        ));

        let instructed: ClientCommand = serde_json::from_value(serde_json::json!({
            "type": "compact",
            "request_id": "compact-instructed",
            "instructions": "Keep the deployment decisions"
        }))
        .unwrap();
        assert!(matches!(
            instructed,
            ClientCommand::Compact {
                request_id,
                instructions: Some(instructions),
            } if request_id == "compact-instructed"
                && instructions == "Keep the deployment decisions"
        ));
    }

    #[test]
    fn compaction_event_serializes_status_without_prompt_or_summary_content() {
        let started = EventDto::from(AgentEvent::ContextCompactionStarted {
            trigger: ContextCompactionTrigger::Manual {
                instructions: Some("Preserve decisions".to_owned()),
            },
            compactor: "test_compactor".to_owned(),
            prompt: "Summarize this conversation".to_owned(),
        });
        let started = serde_json::to_value(started).unwrap();
        assert_eq!(started["type"], "context_compaction_started");
        assert!(started.get("prompt").is_none());
        assert!(!started.to_string().contains("Summarize this conversation"));

        let automatic = serde_json::to_value(ContextCompactionTriggerDto::from(
            ContextCompactionTrigger::Automatic {
                usage: ContextUsage {
                    max_tokens: 200_000,
                    used_tokens: 167_000,
                    remaining_tokens: 33_000,
                },
            },
        ))
        .unwrap();
        assert_eq!(automatic["type"], "automatic");
        assert_eq!(automatic["usage"]["used_tokens"], 167_000);

        let event = EventDto::from(AgentEvent::ContextCompactionCompleted {
            trigger: ContextCompactionTrigger::Manual {
                instructions: Some("Preserve decisions".to_owned()),
            },
            compactor: "test_compactor".to_owned(),
            before_message_count: 4,
            after_message_count: 1,
            changed_from: 0,
            replacement: vec![Message::user("summary")].into(),
            summary: "summary".to_owned(),
            usage: Some(TokenUsage::new(10, 2, 0)),
            estimated_context_tokens: 7,
        });

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "context_compaction_completed");
        assert_eq!(value["trigger"]["type"], "manual");
        assert_eq!(value["trigger"]["instructions"], "Preserve decisions");
        assert_eq!(value["compactor"], "test_compactor");
        assert!(value.get("changed_from").is_none());
        assert!(value.get("replacement").is_none());
        assert!(value.get("summary").is_none());
        assert!(!value.to_string().contains("summary"));
        assert_eq!(value["usage"]["total_tokens"], 12);
        assert_eq!(value["estimated_context_tokens"], 7);

        let overflow = serde_json::to_value(ContextCompactionTriggerDto::from(
            ContextCompactionTrigger::ContextLengthExceeded {
                error: "raw provider response must stay internal".to_owned(),
            },
        ))
        .unwrap();
        assert_eq!(
            overflow,
            serde_json::json!({ "type": "context_length_exceeded" })
        );
    }
}
