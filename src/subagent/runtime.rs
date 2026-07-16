use std::{
    collections::{HashMap, HashSet, VecDeque},
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    Agent, AgentMailbox, AgentMailboxSendError, AgentMailboxSender, AgentRunControl,
    tool::CapabilityMode,
    types::{
        AgentEvent, AgentRunOutcome as CoreAgentRunOutcome, Content, ContextUsage,
        GenerationConfig, Message, TokenUsage,
    },
};

/// Default maximum number of non-closed children owned by one runtime.
pub const DEFAULT_MAX_SUBAGENTS: usize = 4;
/// Default number of parent-to-child messages that may wait for one child.
pub const DEFAULT_SUBAGENT_MAILBOX_CAPACITY: usize = 32;
/// Default maximum UTF-8 byte length of a prompt or notification.
pub const DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES: usize = 64 * 1024;
/// Default number of lifecycle/stream events retained by a broadcast channel.
pub const DEFAULT_SUBAGENT_EVENT_CAPACITY: usize = 512;
/// Maximum number of required top-level fields in a JSON output contract.
pub const MAX_SUBAGENT_OUTPUT_FIELDS: usize = 64;
/// Maximum UTF-8 byte length of one required JSON field name.
pub const MAX_SUBAGENT_OUTPUT_FIELD_BYTES: usize = 128;

/// Built-in coding-agent roles understood by configured subagent factories.
///
/// The library runtime only resolves defaults and carries this value across the
/// factory boundary. A daemon or embedder remains responsible for compiling the
/// corresponding prompt and tool set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentType {
    #[default]
    General,
    Explore,
    Plan,
}

impl SubagentType {
    fn default_capability_mode(self) -> CapabilityMode {
        match self {
            Self::General => CapabilityMode::FullAccess,
            Self::Explore | Self::Plan => CapabilityMode::ReadOnly,
        }
    }
}

/// Workspace placement selected for a configured child.
///
/// `Worktree` is a request to the host factory; the library does not perform
/// Git operations or silently fall back to a shared workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentIsolation {
    #[default]
    Shared,
    Worktree,
}

/// Contract checked against the child's final textual response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubagentOutputContract {
    /// Preserve the legacy behavior: any textual response, including an empty
    /// one, is a valid completion.
    #[default]
    Text,
    /// Require valid JSON and, when listed, top-level object fields.
    Json {
        #[serde(default)]
        required_fields: Vec<String>,
    },
}

impl SubagentOutputContract {
    fn normalized(self) -> Result<Self, String> {
        let Self::Json { required_fields } = self else {
            return Ok(Self::Text);
        };
        if required_fields.len() > MAX_SUBAGENT_OUTPUT_FIELDS {
            return Err(format!(
                "output contract has {} required fields; maximum is {}",
                required_fields.len(),
                MAX_SUBAGENT_OUTPUT_FIELDS
            ));
        }
        let mut normalized = Vec::with_capacity(required_fields.len());
        let mut seen = HashSet::with_capacity(required_fields.len());
        for (index, field) in required_fields.into_iter().enumerate() {
            let field = field.trim();
            if field.is_empty() {
                return Err(format!(
                    "output contract required_fields[{index}] must not be empty"
                ));
            }
            if field.len() > MAX_SUBAGENT_OUTPUT_FIELD_BYTES {
                return Err(format!(
                    "output contract required_fields[{index}] is {} bytes; maximum is {}",
                    field.len(),
                    MAX_SUBAGENT_OUTPUT_FIELD_BYTES
                ));
            }
            if field.chars().any(char::is_control) {
                return Err(format!(
                    "output contract required_fields[{index}] must not contain control characters"
                ));
            }
            if !seen.insert(field.to_owned()) {
                return Err(format!(
                    "output contract required field {field:?} is duplicated"
                ));
            }
            normalized.push(field.to_owned());
        }
        Ok(Self::Json {
            required_fields: normalized,
        })
    }

    fn validate_output(&self, text: &str) -> Result<ValidatedSubagentOutput, String> {
        match self {
            Self::Text => Ok(ValidatedSubagentOutput::Text {
                text: text.to_owned(),
            }),
            Self::Json { required_fields } => {
                let value: Value = serde_json::from_str(text)
                    .map_err(|error| format!("subagent output is not valid JSON: {error}"))?;
                if !required_fields.is_empty() {
                    let object = value.as_object().ok_or_else(|| {
                        "subagent JSON output must be an object when required_fields are configured"
                            .to_owned()
                    })?;
                    let missing = required_fields
                        .iter()
                        .filter(|field| !object.contains_key(field.as_str()))
                        .cloned()
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        return Err(format!(
                            "subagent JSON output is missing required field(s): {}",
                            missing.join(", ")
                        ));
                    }
                }
                Ok(ValidatedSubagentOutput::Json { value })
            }
        }
    }
}

/// Normalized child result after output-contract validation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValidatedSubagentOutput {
    Text { text: String },
    Json { value: Value },
}

/// Fully-resolved, immutable configuration selected for one child.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectiveSubagentConfig {
    pub agent_type: SubagentType,
    pub capability_mode: CapabilityMode,
    pub generation_config: GenerationConfig,
    pub output_contract: SubagentOutputContract,
    pub isolation: SubagentIsolation,
}

#[derive(Clone, Debug)]
pub struct SubagentConfig {
    pub max_agents: usize,
    pub mailbox_capacity: usize,
    pub max_message_bytes: usize,
    pub event_capacity: usize,
    /// Maximum capability that a newly-created child may receive.
    pub capability_ceiling: CapabilityMode,
    /// Generation settings snapshotted into newly-created children.
    pub generation_config: GenerationConfig,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_agents: DEFAULT_MAX_SUBAGENTS,
            mailbox_capacity: DEFAULT_SUBAGENT_MAILBOX_CAPACITY,
            max_message_bytes: DEFAULT_MAX_SUBAGENT_MESSAGE_BYTES,
            event_capacity: DEFAULT_SUBAGENT_EVENT_CAPACITY,
            capability_ceiling: CapabilityMode::FullAccess,
            generation_config: GenerationConfig::default(),
        }
    }
}

impl SubagentConfig {
    fn normalized(mut self) -> Self {
        self.max_agents = self.max_agents.max(1);
        self.mailbox_capacity = self.mailbox_capacity.max(1);
        self.max_message_bytes = self.max_message_bytes.max(1);
        self.event_capacity = self.event_capacity.max(1);
        self
    }
}

#[derive(Clone, Debug)]
pub struct SubagentBuildRequest {
    pub parent_id: String,
    pub agent_id: String,
    pub description: String,
    pub generation_config: GenerationConfig,
    /// Child factories should keep this false unless they deliberately audit
    /// and bound recursive delegation. The built-in runtime never installs
    /// parent-facing subagent tools on a child.
    pub allow_nested_subagents: bool,
}

/// Compatibility-preserving configured build request.
///
/// The original [`SubagentBuildRequest`] remains unchanged for existing
/// factories. New factories override [`SubagentFactory::build_configured`] and
/// must either honor every effective field or reject the build.
#[derive(Clone, Debug)]
pub struct ConfiguredSubagentBuildRequest {
    pub base: SubagentBuildRequest,
    pub effective_config: EffectiveSubagentConfig,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct SubagentFactoryError {
    message: String,
}

impl SubagentFactoryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Host-owned resource associated with a child, such as a Git worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentResourceInfo {
    pub kind: String,
    pub location: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentResourceDisposition {
    Closed,
    RuntimeFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentResourceFinalization {
    pub preserved: bool,
    pub location: Option<String>,
    pub message: Option<String>,
}

#[async_trait]
pub trait SubagentResource: Send + Sync {
    fn info(&self) -> SubagentResourceInfo;

    /// Finalizes this resource exactly once from the runtime's perspective.
    ///
    /// Implementations should be idempotent as a defense against process
    /// recovery. A dirty worktree may return `preserved: true` instead of
    /// deleting user changes.
    async fn finalize(
        &self,
        disposition: SubagentResourceDisposition,
    ) -> Result<SubagentResourceFinalization, SubagentFactoryError>;
}

pub struct BuiltSubagent {
    pub agent: Agent,
    pub resource: Option<Arc<dyn SubagentResource>>,
}

impl BuiltSubagent {
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            resource: None,
        }
    }

    pub fn with_resource(mut self, resource: Arc<dyn SubagentResource>) -> Self {
        self.resource = Some(resource);
        self
    }
}

#[async_trait]
pub trait SubagentFactory: Send + Sync {
    async fn build(&self, request: SubagentBuildRequest) -> Result<Agent, SubagentFactoryError>;

    /// Builds a configured child and optionally transfers ownership of a host
    /// resource to the runtime.
    ///
    /// The returned future may be cancelled when a child is closed while still
    /// starting. Implementations must therefore clean up any resource that has
    /// been reserved but not yet returned inside [`BuiltSubagent`].
    async fn build_configured(
        &self,
        request: ConfiguredSubagentBuildRequest,
    ) -> Result<BuiltSubagent, SubagentFactoryError> {
        let effective = &request.effective_config;
        if effective.agent_type != SubagentType::General {
            return Err(SubagentFactoryError::new(format!(
                "subagent factory does not support configured agent type {:?}",
                effective.agent_type
            )));
        }
        if effective.capability_mode != CapabilityMode::FullAccess {
            return Err(SubagentFactoryError::new(format!(
                "subagent factory does not support capability mode {:?}",
                effective.capability_mode
            )));
        }
        if effective.isolation != SubagentIsolation::Shared {
            return Err(SubagentFactoryError::new(
                "subagent factory does not support worktree isolation",
            ));
        }
        self.build(request.base).await.map(BuiltSubagent::new)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentState {
    Starting,
    Running,
    Idle,
    Closing,
    Closed,
}

impl SubagentState {
    fn as_u8(&self) -> u8 {
        match self {
            Self::Starting => 0,
            Self::Running => 1,
            Self::Idle => 2,
            Self::Closing => 3,
            Self::Closed => 4,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Starting,
            1 => Self::Running,
            2 => Self::Idle,
            3 => Self::Closing,
            _ => Self::Closed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentNotificationKind {
    Progress,
    Blocker,
    Result,
    Failed,
    Closed,
}

impl SubagentNotificationKind {
    pub fn wakes_parent(&self) -> bool {
        !matches!(self, Self::Progress)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentNotificationSource {
    Child,
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentNotification {
    pub delivery_id: String,
    pub kind: SubagentNotificationKind,
    pub source: SubagentNotificationSource,
    pub message: String,
    pub wake_parent: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SubagentRunOutcome {
    Completed {
        text: String,
        turns: usize,
        usage: TokenUsage,
    },
    Stopped,
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSubagentRun {
    pub run_id: String,
    pub delivery_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentSnapshot {
    pub parent_id: String,
    pub agent_id: String,
    pub description: String,
    pub effective_config: EffectiveSubagentConfig,
    pub state: SubagentState,
    pub active_run: Option<ActiveSubagentRun>,
    pub messages: Vec<Message>,
    /// Provider-neutral text accumulated from the current streamed message.
    pub draft: Option<String>,
    pub cumulative_usage: TokenUsage,
    pub context_usage: Option<ContextUsage>,
    pub last_outcome: Option<SubagentRunOutcome>,
    pub validated_output: Option<ValidatedSubagentOutput>,
    pub resource: Option<SubagentResourceInfo>,
    pub resource_finalization: Option<SubagentResourceFinalization>,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentEvent {
    pub sequence: u64,
    pub parent_id: String,
    pub agent_id: String,
    pub kind: SubagentEventKind,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum SubagentEventKind {
    /// Published synchronously before `spawn_agent` returns.
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
    Notification(SubagentNotification),
    /// The complete child stream for read-only daemon observers.
    AgentEvent(AgentEvent),
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
    /// Emitted exactly once for a permanently-closed child.
    Closed {
        delivery_id: String,
        reason: String,
        wake_parent: bool,
    },
}

impl SubagentEvent {
    pub fn wakes_parent(&self) -> bool {
        match &self.kind {
            SubagentEventKind::Notification(notification) => notification.wake_parent,
            SubagentEventKind::Closed { wake_parent, .. } => *wake_parent,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpawnAgentRequest {
    pub description: String,
    pub prompt: String,
    /// Overrides the runtime defaults for this child. Factories may further
    /// restrict these settings.
    pub generation_config: Option<GenerationConfig>,
}

impl SpawnAgentRequest {
    pub fn new(description: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            prompt: prompt.into(),
            generation_config: None,
        }
    }

    pub fn generation_config(mut self, config: GenerationConfig) -> Self {
        self.generation_config = Some(config);
        self
    }
}

/// Extended child-spawn request used by configurable runtimes and the
/// model-facing `spawn_agent` tool.
#[derive(Clone, Debug)]
pub struct ConfiguredSpawnAgentRequest {
    pub description: String,
    pub prompt: String,
    pub agent_type: SubagentType,
    /// Optional restriction layered on top of the type default and runtime
    /// ceiling. It can never widen `explore` or `plan`.
    pub capability_mode: Option<CapabilityMode>,
    pub generation_config: Option<GenerationConfig>,
    pub output_contract: Option<SubagentOutputContract>,
    pub isolation: Option<SubagentIsolation>,
}

impl ConfiguredSpawnAgentRequest {
    pub fn new(description: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            prompt: prompt.into(),
            agent_type: SubagentType::General,
            capability_mode: None,
            generation_config: None,
            output_contract: None,
            isolation: None,
        }
    }

    pub fn agent_type(mut self, agent_type: SubagentType) -> Self {
        self.agent_type = agent_type;
        self
    }

    pub fn capability_mode(mut self, capability_mode: CapabilityMode) -> Self {
        self.capability_mode = Some(capability_mode);
        self
    }

    pub fn generation_config(mut self, generation_config: GenerationConfig) -> Self {
        self.generation_config = Some(generation_config);
        self
    }

    pub fn output_contract(mut self, output_contract: SubagentOutputContract) -> Self {
        self.output_contract = Some(output_contract);
        self
    }

    pub fn isolation(mut self, isolation: SubagentIsolation) -> Self {
        self.isolation = Some(isolation);
        self
    }
}

impl From<SpawnAgentRequest> for ConfiguredSpawnAgentRequest {
    fn from(request: SpawnAgentRequest) -> Self {
        Self {
            description: request.description,
            prompt: request.prompt,
            agent_type: SubagentType::General,
            capability_mode: None,
            generation_config: request.generation_config,
            output_contract: None,
            isolation: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnedSubagent {
    pub agent_id: String,
    pub delivery_id: String,
    pub effective_config: EffectiveSubagentConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedSubagentMessage {
    pub agent_id: String,
    pub delivery_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseSubagentResult {
    pub agent_id: String,
    pub already_closed: bool,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubagentError {
    #[error("subagent runtime is shutting down")]
    ShuttingDown,
    #[error("subagent limit reached ({limit})")]
    LimitReached { limit: usize },
    #[error("subagent `{agent_id}` was not found")]
    NotFound { agent_id: String },
    #[error("subagent `{agent_id}` is closing or permanently closed")]
    Closed { agent_id: String },
    #[error("subagent `{agent_id}` message queue is full")]
    QueueFull { agent_id: String },
    #[error("message must not be empty")]
    EmptyMessage,
    #[error("message is {actual} bytes; maximum is {maximum} bytes")]
    MessageTooLong { actual: usize, maximum: usize },
    #[error("invalid subagent configuration: {message}")]
    InvalidConfiguration { message: String },
}

#[derive(Clone)]
pub struct SubagentRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    parent_id: String,
    factory: Arc<dyn SubagentFactory>,
    config: SubagentConfig,
    capability_ceiling: AtomicU8,
    agents: RwLock<HashMap<String, Arc<SubagentEntry>>>,
    spawn_lock: Mutex<()>,
    events: broadcast::Sender<SubagentEvent>,
    next_sequence: AtomicU64,
    shutting_down: AtomicBool,
}

struct SubagentEntry {
    agent_id: String,
    prebuild_messages: mpsc::Sender<PendingMessage>,
    mailbox: Mutex<Option<AgentMailboxSender>>,
    delivery_ids: Mutex<VecDeque<String>>,
    close: watch::Sender<Option<String>>,
    state: AtomicU8,
    state_watch: watch::Sender<SubagentState>,
    send_close_lock: Mutex<()>,
    active_control: Mutex<Option<AgentRunControl>>,
    snapshot: RwLock<SubagentSnapshot>,
    closed_event_emitted: AtomicBool,
    resource: Mutex<Option<Arc<dyn SubagentResource>>>,
    resource_finalized: AtomicBool,
}

#[derive(Clone, Debug)]
struct PendingMessage {
    delivery_id: String,
    message: String,
}

impl SubagentRuntime {
    pub fn new(
        parent_id: impl Into<String>,
        factory: Arc<dyn SubagentFactory>,
        config: SubagentConfig,
    ) -> Self {
        let config = config.normalized();
        let (events, _) = broadcast::channel(config.event_capacity);
        Self {
            inner: Arc::new(RuntimeInner {
                parent_id: parent_id.into(),
                factory,
                capability_ceiling: AtomicU8::new(capability_mode_to_u8(config.capability_ceiling)),
                config,
                agents: RwLock::new(HashMap::new()),
                spawn_lock: Mutex::new(()),
                events,
                next_sequence: AtomicU64::new(0),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    pub fn parent_id(&self) -> &str {
        &self.inner.parent_id
    }

    pub fn config(&self) -> &SubagentConfig {
        &self.inner.config
    }

    pub fn capability_ceiling(&self) -> CapabilityMode {
        capability_mode_from_u8(self.inner.capability_ceiling.load(Ordering::Acquire))
    }

    /// Changes the ceiling used for future configured children.
    ///
    /// Owners that narrow this value should first close children returned by
    /// [`Self::close_exceeding_capability`] so already-running work cannot keep
    /// a broader authority than the parent session.
    pub fn set_capability_ceiling(&self, capability_mode: CapabilityMode) {
        self.inner
            .capability_ceiling
            .store(capability_mode_to_u8(capability_mode), Ordering::Release);
    }

    pub async fn close_exceeding_capability(
        &self,
        ceiling: CapabilityMode,
        reason: impl Into<String>,
    ) {
        // Narrow future reservations first. A child whose spawn races this
        // transition will therefore be resolved against the new ceiling even
        // if it has not yet appeared in the snapshot below.
        self.set_capability_ceiling(ceiling);
        let reason = reason.into();
        let ids = self
            .snapshots()
            .into_iter()
            .filter(|snapshot| {
                !snapshot
                    .effective_config
                    .capability_mode
                    .is_subset_of(ceiling)
                    && !matches!(
                        snapshot.state,
                        SubagentState::Closing | SubagentState::Closed
                    )
            })
            .map(|snapshot| snapshot.agent_id)
            .collect::<Vec<_>>();
        for agent_id in ids {
            let _ = self.close(&agent_id, reason.clone()).await;
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.inner.events.subscribe()
    }

    pub fn snapshot(&self, agent_id: &str) -> Option<SubagentSnapshot> {
        self.entry(agent_id)
            .ok()
            .map(|entry| read_lock(&entry.snapshot).clone())
    }

    pub fn snapshots(&self) -> Vec<SubagentSnapshot> {
        let mut snapshots = read_lock(&self.inner.agents)
            .values()
            .map(|entry| read_lock(&entry.snapshot).clone())
            .collect::<Vec<_>>();
        snapshots.sort_unstable_by(|left, right| left.agent_id.cmp(&right.agent_id));
        snapshots
    }

    pub async fn spawn(
        &self,
        request: SpawnAgentRequest,
    ) -> Result<SpawnedSubagent, SubagentError> {
        self.spawn_configured(request.into()).await
    }

    pub async fn spawn_configured(
        &self,
        request: ConfiguredSpawnAgentRequest,
    ) -> Result<SpawnedSubagent, SubagentError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(SubagentError::ShuttingDown);
        }
        self.validate_text(&request.prompt)?;
        if request.description.trim().is_empty() {
            return Err(SubagentError::EmptyMessage);
        }
        self.validate_text(&request.description)?;
        let output_contract = request
            .output_contract
            .unwrap_or_default()
            .normalized()
            .map_err(|message| SubagentError::InvalidConfiguration { message })?;
        let type_ceiling = request.agent_type.default_capability_mode();
        let generation_config = request.generation_config.map_or_else(
            || self.inner.config.generation_config.clone(),
            |overrides| merge_generation_config(&self.inner.config.generation_config, overrides),
        );

        // Reserve capacity and insert the Starting tombstone under one lock;
        // concurrent callers cannot both pass the max-agents check. Resolve
        // the capability inside the same critical section so a concurrent
        // parent downgrade cannot admit a child using a stale ceiling.
        let spawn_guard = mutex_lock(&self.inner.spawn_lock);
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(SubagentError::ShuttingDown);
        }
        let capability_mode = request
            .capability_mode
            .unwrap_or(type_ceiling)
            .safest(type_ceiling)
            .safest(self.capability_ceiling());
        let effective_config = EffectiveSubagentConfig {
            agent_type: request.agent_type,
            capability_mode,
            generation_config,
            output_contract,
            isolation: request.isolation.unwrap_or_default(),
        };

        let (message_sender, message_receiver) = mpsc::channel(self.inner.config.mailbox_capacity);
        let (close, _) = watch::channel(None);
        let (state_watch, _) = watch::channel(SubagentState::Starting);
        let agent_id = self.reserve_agent_id()?;
        let initial_delivery_id = new_id("delivery");
        let snapshot = SubagentSnapshot {
            parent_id: self.inner.parent_id.clone(),
            agent_id: agent_id.clone(),
            description: request.description.clone(),
            effective_config: effective_config.clone(),
            state: SubagentState::Starting,
            active_run: None,
            messages: Vec::new(),
            draft: None,
            cumulative_usage: TokenUsage::default(),
            context_usage: None,
            last_outcome: None,
            validated_output: None,
            resource: None,
            resource_finalization: None,
            last_sequence: 0,
        };
        let entry = Arc::new(SubagentEntry {
            agent_id: agent_id.clone(),
            prebuild_messages: message_sender,
            mailbox: Mutex::new(None),
            delivery_ids: Mutex::new(VecDeque::from([initial_delivery_id.clone()])),
            close,
            state: AtomicU8::new(SubagentState::Starting.as_u8()),
            state_watch,
            send_close_lock: Mutex::new(()),
            active_control: Mutex::new(None),
            snapshot: RwLock::new(snapshot),
            closed_event_emitted: AtomicBool::new(false),
            resource: Mutex::new(None),
            resource_finalized: AtomicBool::new(false),
        });
        write_lock(&self.inner.agents).insert(agent_id.clone(), entry.clone());
        drop(spawn_guard);

        // This ordering is an API guarantee used by the daemon to tell the
        // parent caller about delegation before the model receives tool output.
        self.publish(
            &entry,
            SubagentEventKind::Spawned {
                description: request.description.clone(),
                initial_delivery_id: initial_delivery_id.clone(),
                effective_config: effective_config.clone(),
            },
        );

        let build_request = ConfiguredSubagentBuildRequest {
            base: SubagentBuildRequest {
                parent_id: self.inner.parent_id.clone(),
                agent_id: agent_id.clone(),
                description: request.description,
                generation_config: effective_config.generation_config.clone(),
                allow_nested_subagents: false,
            },
            effective_config: effective_config.clone(),
        };
        let initial = PendingMessage {
            delivery_id: initial_delivery_id.clone(),
            message: request.prompt,
        };
        let runtime = self.clone();
        tokio::spawn(async move {
            let worker =
                runtime.run_worker(entry.clone(), build_request, message_receiver, initial);
            if AssertUnwindSafe(worker).catch_unwind().await.is_err() {
                runtime.finish_after_panic(&entry).await;
            }
        });

        Ok(SpawnedSubagent {
            agent_id,
            delivery_id: initial_delivery_id,
            effective_config,
        })
    }

    pub fn send_message(
        &self,
        agent_id: &str,
        message: impl Into<String>,
    ) -> Result<QueuedSubagentMessage, SubagentError> {
        let message = message.into();
        self.validate_text(&message)?;
        let entry = self.entry(agent_id)?;
        let _coordination = mutex_lock(&entry.send_close_lock);
        if matches!(
            entry.state(),
            SubagentState::Closing | SubagentState::Closed
        ) {
            return Err(SubagentError::Closed {
                agent_id: agent_id.to_owned(),
            });
        }
        let delivery_id = new_id("delivery");
        if let Some(mailbox) = mutex_lock(&entry.mailbox).as_ref() {
            mailbox
                .send(Content::text(message))
                .map_err(|error| map_mailbox_error(agent_id, error))?;
        } else {
            entry
                .prebuild_messages
                .try_send(PendingMessage {
                    delivery_id: delivery_id.clone(),
                    message,
                })
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => SubagentError::QueueFull {
                        agent_id: agent_id.to_owned(),
                    },
                    mpsc::error::TrySendError::Closed(_) => SubagentError::Closed {
                        agent_id: agent_id.to_owned(),
                    },
                })?;
        }
        mutex_lock(&entry.delivery_ids).push_back(delivery_id.clone());
        self.publish(
            &entry,
            SubagentEventKind::MessageQueued {
                delivery_id: delivery_id.clone(),
            },
        );
        Ok(QueuedSubagentMessage {
            agent_id: agent_id.to_owned(),
            delivery_id,
        })
    }

    pub async fn close(
        &self,
        agent_id: &str,
        reason: impl Into<String>,
    ) -> Result<CloseSubagentResult, SubagentError> {
        let entry = self.entry(agent_id)?;
        let reason = reason.into();
        let already_closed = {
            let _coordination = mutex_lock(&entry.send_close_lock);
            match entry.state() {
                SubagentState::Closed => true,
                SubagentState::Closing => false,
                _ => {
                    let reason = if reason.trim().is_empty() {
                        "explicitly closed".to_owned()
                    } else {
                        self.validate_text(&reason)?;
                        reason
                    };
                    entry.set_state(SubagentState::Closing);
                    self.publish(
                        &entry,
                        SubagentEventKind::StateChanged {
                            state: SubagentState::Closing,
                        },
                    );
                    if let Some(control) = mutex_lock(&entry.active_control).as_ref() {
                        control.stop();
                    }
                    if let Some(mailbox) = mutex_lock(&entry.mailbox).as_ref() {
                        mailbox.close();
                    }
                    entry.close.send_replace(Some(reason));
                    false
                }
            }
        };
        if !already_closed {
            entry.wait_closed().await;
        }
        Ok(CloseSubagentResult {
            agent_id: agent_id.to_owned(),
            already_closed,
        })
    }

    /// Permanently rejects new spawns and closes every child.
    pub async fn shutdown(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let ids = {
            // Paired with spawn's reservation lock: either the child is
            // inserted before this snapshot and is closed below, or its
            // second shutting_down check rejects it.
            let _spawn_guard = mutex_lock(&self.inner.spawn_lock);
            self.inner.shutting_down.store(true, Ordering::Release);
            read_lock(&self.inner.agents)
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        for agent_id in ids {
            let _ = self.close(&agent_id, reason.clone()).await;
        }
    }

    pub(crate) fn notify(
        &self,
        agent_id: &str,
        kind: SubagentNotificationKind,
        message: String,
        source: SubagentNotificationSource,
    ) -> Result<String, SubagentError> {
        self.validate_text(&message)?;
        let entry = self.entry(agent_id)?;
        let _coordination = mutex_lock(&entry.send_close_lock);
        if matches!(
            entry.state(),
            SubagentState::Closing | SubagentState::Closed
        ) {
            return Err(SubagentError::Closed {
                agent_id: agent_id.to_owned(),
            });
        }
        let delivery_id = new_id("delivery");
        let wake_parent = kind.wakes_parent();
        self.publish(
            &entry,
            SubagentEventKind::Notification(SubagentNotification {
                delivery_id: delivery_id.clone(),
                kind,
                source,
                message,
                wake_parent,
            }),
        );
        Ok(delivery_id)
    }

    async fn run_worker(
        &self,
        entry: Arc<SubagentEntry>,
        build_request: ConfiguredSubagentBuildRequest,
        mut message_receiver: mpsc::Receiver<PendingMessage>,
        initial: PendingMessage,
    ) {
        let mut close_receiver = entry.close.subscribe();
        let build = self.inner.factory.build_configured(build_request);
        tokio::pin!(build);
        let built = tokio::select! {
            biased;
            _ = wait_for_close(&mut close_receiver) => {
                self.finish_closed(
                    &entry,
                    close_reason(&close_receiver),
                    SubagentResourceDisposition::Closed,
                ).await;
                return;
            }
            result = &mut build => match result {
                Ok(built) => built,
                Err(error) => {
                    let message = format!("failed to build subagent: {error}");
                    self.record_failed(&entry, "build", message.clone());
                    self.finish_closed(
                        &entry,
                        message,
                        SubagentResourceDisposition::RuntimeFailed,
                    ).await;
                    return;
                }
            }
        };
        let BuiltSubagent {
            mut agent,
            resource,
        } = built;
        if let Some(resource) = resource {
            *mutex_lock(&entry.resource) = Some(Arc::clone(&resource));
            let info = resource.info();
            write_lock(&entry.snapshot).resource = Some(info);
        }

        agent.add_mandatory_tool(crate::tool::subagent::NotifyParentTool::new(
            self.clone(),
            entry.agent_id.clone(),
        ));
        let weak_runtime = Arc::downgrade(&self.inner);
        let weak_entry = Arc::downgrade(&entry);
        agent.subscribe(move |event| {
            forward_agent_event(&weak_runtime, &weak_entry, event);
        });

        // During construction messages wait in a small pre-build channel.
        // Switch atomically to Agent's mailbox so messages sent while a run is
        // active are injected at the next protocol-safe boundary.
        let (mailbox_sender, mailbox_receiver) =
            AgentMailbox::bounded(self.inner.config.mailbox_capacity.saturating_add(1));
        agent.set_mailbox(mailbox_receiver);
        let setup_failure = {
            let _coordination = mutex_lock(&entry.send_close_lock);
            if matches!(
                entry.state(),
                SubagentState::Closing | SubagentState::Closed
            ) {
                mailbox_sender.close();
                Some((
                    close_reason(&close_receiver),
                    SubagentResourceDisposition::Closed,
                    false,
                ))
            } else if mailbox_sender.send(Content::text(initial.message)).is_err() {
                Some((
                    "failed to queue initial subagent prompt".to_owned(),
                    SubagentResourceDisposition::RuntimeFailed,
                    false,
                ))
            } else {
                let mut failure = None;
                while let Ok(pending) = message_receiver.try_recv() {
                    let _delivery_id = pending.delivery_id;
                    if mailbox_sender.send(Content::text(pending.message)).is_err() {
                        failure = Some((
                            "subagent mailbox transfer failed".to_owned(),
                            SubagentResourceDisposition::RuntimeFailed,
                            true,
                        ));
                        break;
                    }
                }
                if failure.is_none() {
                    *mutex_lock(&entry.mailbox) = Some(mailbox_sender.clone());
                }
                failure
            }
        };
        if let Some((reason, disposition, record_failure)) = setup_failure {
            if record_failure {
                self.record_failed(&entry, "mailbox", reason.clone());
            }
            self.finish_closed(&entry, reason, disposition).await;
            return;
        }

        self.transition_operational(&entry, SubagentState::Idle);
        loop {
            let wake = tokio::select! {
                biased;
                _ = wait_for_close(&mut close_receiver) => false,
                wake = mailbox_sender.wait_for_wake() => wake,
            };
            if !wake || close_receiver.borrow().is_some() {
                self.finish_closed(
                    &entry,
                    close_reason(&close_receiver),
                    SubagentResourceDisposition::Closed,
                )
                .await;
                return;
            }
            let run_id = new_id("run");
            let delivery_id = {
                // A sender holds this lock through both mailbox acceptance and
                // delivery-id bookkeeping, so wake cannot expose a synthetic
                // ID for a message whose real ID is still being recorded.
                let _coordination = mutex_lock(&entry.send_close_lock);
                mutex_lock(&entry.delivery_ids)
                    .front()
                    .cloned()
                    .unwrap_or_else(|| new_id("delivery"))
            };
            {
                let mut snapshot = write_lock(&entry.snapshot);
                snapshot.active_run = Some(ActiveSubagentRun {
                    run_id: run_id.clone(),
                    delivery_id,
                });
                snapshot.draft = None;
                snapshot.validated_output = None;
            }
            self.transition_operational(&entry, SubagentState::Running);
            let control = AgentRunControl::new();
            *mutex_lock(&entry.active_control) = Some(control.clone());
            let run = agent.prompt_from_mailbox_controlled(control.clone());
            tokio::pin!(run);

            let outcome = tokio::select! {
                biased;
                _ = wait_for_close(&mut close_receiver) => {
                    control.stop();
                    (&mut run).await
                }
                outcome = &mut run => outcome,
            };
            *mutex_lock(&entry.active_control) = None;
            write_lock(&entry.snapshot).active_run = None;

            if close_receiver.borrow().is_some() {
                self.finish_closed(
                    &entry,
                    close_reason(&close_receiver),
                    SubagentResourceDisposition::Closed,
                )
                .await;
                return;
            }
            match outcome {
                Ok(Some(CoreAgentRunOutcome::Completed(run))) => {
                    let text = run.text().unwrap_or_default().to_owned();
                    let output_contract = read_lock(&entry.snapshot)
                        .effective_config
                        .output_contract
                        .clone();
                    match output_contract.validate_output(&text) {
                        Ok(output) => {
                            let outcome = SubagentRunOutcome::Completed {
                                text: text.clone(),
                                turns: run.turns,
                                usage: run.run_usage,
                            };
                            {
                                let mut snapshot = write_lock(&entry.snapshot);
                                snapshot.last_outcome = Some(outcome.clone());
                                snapshot.validated_output = Some(output.clone());
                            }
                            self.publish(&entry, SubagentEventKind::OutputValidated { output });
                            self.publish(
                                &entry,
                                SubagentEventKind::RunFinished {
                                    run_id: run_id.clone(),
                                    outcome,
                                },
                            );
                            let _ = self.notify(
                                &entry.agent_id,
                                SubagentNotificationKind::Result,
                                if text.is_empty() {
                                    "Subagent completed without a textual result.".to_owned()
                                } else {
                                    text
                                },
                                SubagentNotificationSource::Runtime,
                            );
                        }
                        Err(error) => {
                            self.record_failed(
                                &entry,
                                &run_id,
                                format!("output contract violation: {error}"),
                            );
                        }
                    }
                }
                Ok(Some(CoreAgentRunOutcome::Stopped)) => {
                    let outcome = SubagentRunOutcome::Stopped;
                    {
                        let mut snapshot = write_lock(&entry.snapshot);
                        snapshot.last_outcome = Some(outcome.clone());
                        snapshot.validated_output = None;
                    }
                    self.publish(&entry, SubagentEventKind::RunFinished { run_id, outcome });
                }
                Ok(None) => {
                    self.transition_operational(&entry, SubagentState::Idle);
                    continue;
                }
                Err(error) => {
                    let message = error.to_string();
                    self.record_failed(&entry, &run_id, message);
                }
            }
            // Retain only IDs for mailbox messages that were not durably
            // committed. The coordination lock pairs this with send_message.
            {
                let _coordination = mutex_lock(&entry.send_close_lock);
                let pending = mailbox_sender.pending_len();
                let mut delivery_ids = mutex_lock(&entry.delivery_ids);
                while delivery_ids.len() > pending {
                    delivery_ids.pop_front();
                }
            }
            self.transition_operational(&entry, SubagentState::Idle);
        }
    }

    fn record_failed(&self, entry: &Arc<SubagentEntry>, run_id: &str, message: String) {
        let outcome = SubagentRunOutcome::Failed {
            error: message.clone(),
        };
        {
            let mut snapshot = write_lock(&entry.snapshot);
            snapshot.last_outcome = Some(outcome.clone());
            snapshot.validated_output = None;
        }
        self.publish(
            entry,
            SubagentEventKind::RunFinished {
                run_id: run_id.to_owned(),
                outcome,
            },
        );
        let _ = self.notify(
            &entry.agent_id,
            SubagentNotificationKind::Failed,
            message,
            SubagentNotificationSource::Runtime,
        );
    }

    fn transition_operational(&self, entry: &Arc<SubagentEntry>, state: SubagentState) {
        let current = entry.state();
        if matches!(current, SubagentState::Closing | SubagentState::Closed) {
            return;
        }
        entry.set_state(state.clone());
        self.publish(entry, SubagentEventKind::StateChanged { state });
    }

    async fn finish_after_panic(&self, entry: &Arc<SubagentEntry>) {
        let message = "subagent worker panicked".to_owned();
        self.record_failed(entry, "worker", message.clone());
        self.finish_closed(entry, message, SubagentResourceDisposition::RuntimeFailed)
            .await;
    }

    async fn finish_closed(
        &self,
        entry: &Arc<SubagentEntry>,
        mut reason: String,
        disposition: SubagentResourceDisposition,
    ) {
        self.finalize_resource(entry, disposition).await;
        if let Some(finalization) = read_lock(&entry.snapshot)
            .resource_finalization
            .as_ref()
            .filter(|finalization| finalization.preserved)
        {
            let detail = match (
                finalization.message.as_deref(),
                finalization.location.as_deref(),
            ) {
                (Some(message), Some(location)) => format!("{message} at {location}"),
                (Some(message), None) => message.to_owned(),
                (None, Some(location)) => format!("resource preserved at {location}"),
                (None, None) => "resource preserved for inspection".to_owned(),
            };
            reason = format!("{reason}; {detail}");
        }
        entry.set_state(SubagentState::Closed);
        {
            let mut snapshot = write_lock(&entry.snapshot);
            snapshot.active_run = None;
            snapshot.draft = None;
        }
        if !entry.closed_event_emitted.swap(true, Ordering::AcqRel) {
            self.publish(
                entry,
                SubagentEventKind::Closed {
                    delivery_id: new_id("delivery"),
                    reason,
                    wake_parent: true,
                },
            );
        }
    }

    async fn finalize_resource(
        &self,
        entry: &Arc<SubagentEntry>,
        disposition: SubagentResourceDisposition,
    ) {
        if entry.resource_finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        let resource = { mutex_lock(&entry.resource).take() };
        let Some(resource) = resource else {
            return;
        };
        let result = AssertUnwindSafe(resource.finalize(disposition))
            .catch_unwind()
            .await;
        match result {
            Ok(Ok(finalization)) => {
                write_lock(&entry.snapshot).resource_finalization = Some(finalization.clone());
                self.publish(entry, SubagentEventKind::ResourceFinalized { finalization });
            }
            Ok(Err(error)) => {
                self.publish(
                    entry,
                    SubagentEventKind::ResourceFinalizationFailed {
                        error: error.to_string(),
                    },
                );
            }
            Err(_) => {
                self.publish(
                    entry,
                    SubagentEventKind::ResourceFinalizationFailed {
                        error: "subagent resource finalizer panicked".to_owned(),
                    },
                );
            }
        }
    }

    fn reserve_agent_id(&self) -> Result<String, SubagentError> {
        let agents = read_lock(&self.inner.agents);
        let active = agents
            .values()
            .filter(|entry| entry.state() != SubagentState::Closed)
            .count();
        if active >= self.inner.config.max_agents {
            return Err(SubagentError::LimitReached {
                limit: self.inner.config.max_agents,
            });
        }
        loop {
            let candidate = new_id("agent");
            if !agents.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
    }

    fn entry(&self, agent_id: &str) -> Result<Arc<SubagentEntry>, SubagentError> {
        read_lock(&self.inner.agents)
            .get(agent_id)
            .cloned()
            .ok_or_else(|| SubagentError::NotFound {
                agent_id: agent_id.to_owned(),
            })
    }

    fn validate_text(&self, message: &str) -> Result<(), SubagentError> {
        if message.trim().is_empty() {
            return Err(SubagentError::EmptyMessage);
        }
        let actual = message.len();
        if actual > self.inner.config.max_message_bytes {
            return Err(SubagentError::MessageTooLong {
                actual,
                maximum: self.inner.config.max_message_bytes,
            });
        }
        Ok(())
    }

    fn publish(&self, entry: &Arc<SubagentEntry>, kind: SubagentEventKind) -> u64 {
        publish_inner(&self.inner, entry, kind)
    }
}

impl SubagentEntry {
    fn state(&self) -> SubagentState {
        SubagentState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: SubagentState) {
        self.state.store(state.as_u8(), Ordering::Release);
        write_lock(&self.snapshot).state = state.clone();
        self.state_watch.send_replace(state);
    }

    async fn wait_closed(&self) {
        if self.state() == SubagentState::Closed {
            return;
        }
        let mut receiver = self.state_watch.subscribe();
        while *receiver.borrow_and_update() != SubagentState::Closed {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

fn forward_agent_event(
    runtime: &Weak<RuntimeInner>,
    entry: &Weak<SubagentEntry>,
    event: &AgentEvent,
) {
    let (Some(runtime), Some(entry)) = (runtime.upgrade(), entry.upgrade()) else {
        return;
    };
    {
        let mut snapshot = write_lock(&entry.snapshot);
        match event {
            AgentEvent::MessageStart { message }
                if message.role == crate::types::Role::Assistant =>
            {
                snapshot.draft = Some(String::new());
            }
            AgentEvent::MessageUpdate {
                delta: crate::types::AssistantDelta::Text { delta },
            } => {
                snapshot
                    .draft
                    .get_or_insert_with(String::new)
                    .push_str(delta);
            }
            AgentEvent::MessageEnd { .. } | AgentEvent::MessageAborted => {
                snapshot.draft = None;
            }
            AgentEvent::AgentEnd { messages } | AgentEvent::AgentStopped { messages } => {
                snapshot.messages = messages.clone();
                snapshot.draft = None;
            }
            AgentEvent::UsageUpdate {
                usage,
                context_usage,
            } => {
                snapshot.cumulative_usage += *usage;
                snapshot.context_usage = *context_usage;
            }
            _ => {}
        }
    }
    publish_inner(
        &runtime,
        &entry,
        SubagentEventKind::AgentEvent(event.clone()),
    );
}

fn publish_inner(
    runtime: &Arc<RuntimeInner>,
    entry: &Arc<SubagentEntry>,
    kind: SubagentEventKind,
) -> u64 {
    let sequence = runtime.next_sequence.fetch_add(1, Ordering::AcqRel) + 1;
    write_lock(&entry.snapshot).last_sequence = sequence;
    let _ = runtime.events.send(SubagentEvent {
        sequence,
        parent_id: runtime.parent_id.clone(),
        agent_id: entry.agent_id.clone(),
        kind,
    });
    sequence
}

async fn wait_for_close(receiver: &mut watch::Receiver<Option<String>>) {
    loop {
        if receiver.borrow_and_update().is_some() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

fn close_reason(receiver: &watch::Receiver<Option<String>>) -> String {
    receiver
        .borrow()
        .clone()
        .unwrap_or_else(|| "subagent runtime stopped".to_owned())
}

fn map_mailbox_error(agent_id: &str, error: AgentMailboxSendError) -> SubagentError {
    match error {
        AgentMailboxSendError::Full { .. } => SubagentError::QueueFull {
            agent_id: agent_id.to_owned(),
        },
        AgentMailboxSendError::Closed => SubagentError::Closed {
            agent_id: agent_id.to_owned(),
        },
    }
}

fn merge_generation_config(
    defaults: &GenerationConfig,
    overrides: GenerationConfig,
) -> GenerationConfig {
    GenerationConfig {
        model: overrides.model.or_else(|| defaults.model.clone()),
        temperature: overrides.temperature.or(defaults.temperature),
        max_tokens: overrides.max_tokens.or(defaults.max_tokens),
        reasoning_effort: overrides.reasoning_effort.or(defaults.reasoning_effort),
    }
}

fn capability_mode_to_u8(mode: CapabilityMode) -> u8 {
    match mode {
        CapabilityMode::ReadOnly => 0,
        CapabilityMode::WorkspaceEdit => 1,
        CapabilityMode::FullAccess => 2,
    }
}

fn capability_mode_from_u8(value: u8) -> CapabilityMode {
    match value {
        0 => CapabilityMode::ReadOnly,
        1 => CapabilityMode::WorkspaceEdit,
        _ => CapabilityMode::FullAccess,
    }
}

fn new_id(prefix: &str) -> String {
    format!(
        "{prefix}_{:016x}{:016x}",
        fastrand::u64(..),
        fastrand::u64(..)
    )
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mutex_lock<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use crate::{
        Agent,
        provider::{LlmProvider, ProviderEventStream},
        types::{
            AssistantMessage, Message, ProviderEvent, ProviderRequest, ProviderResponse, Role,
            TokenUsage,
        },
    };
    use tokio::sync::{Barrier, Notify};

    use super::*;

    struct PendingFactory {
        entered: Arc<Notify>,
    }

    #[async_trait]
    impl SubagentFactory for PendingFactory {
        async fn build(
            &self,
            _request: SubagentBuildRequest,
        ) -> Result<Agent, SubagentFactoryError> {
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    #[derive(Default)]
    struct ProviderState {
        requests: Mutex<Vec<ProviderRequest>>,
        first_started: Notify,
        release_first: Notify,
        calls: AtomicUsize,
    }

    struct PausedProvider {
        state: Arc<ProviderState>,
    }

    impl LlmProvider for PausedProvider {
        fn stream(&self, request: ProviderRequest) -> ProviderEventStream {
            mutex_lock(&self.state.requests).push(request);
            if self.state.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let state = Arc::clone(&self.state);
                Box::pin(async_stream::stream! {
                    state.first_started.notify_one();
                    state.release_first.notified().await;
                    yield Ok(ProviderEvent::Done(response("first response")));
                })
            } else {
                Box::pin(futures_util::stream::iter([Ok(ProviderEvent::Done(
                    response("response after steering"),
                ))]))
            }
        }
    }

    struct ProviderFactory {
        state: Arc<ProviderState>,
    }

    #[async_trait]
    impl SubagentFactory for ProviderFactory {
        async fn build(
            &self,
            request: SubagentBuildRequest,
        ) -> Result<Agent, SubagentFactoryError> {
            assert!(!request.allow_nested_subagents);
            Ok(Agent::builder(PausedProvider {
                state: Arc::clone(&self.state),
            })
            .generation_config(request.generation_config)
            .build())
        }
    }

    #[derive(Clone)]
    enum TestProviderBehavior {
        Response(String),
        Panic,
        Pending,
    }

    struct TestProvider {
        behavior: TestProviderBehavior,
    }

    impl LlmProvider for TestProvider {
        fn stream(&self, _request: ProviderRequest) -> ProviderEventStream {
            match &self.behavior {
                TestProviderBehavior::Response(text) => {
                    let response = response(text);
                    Box::pin(futures_util::stream::iter([Ok(ProviderEvent::Done(
                        response,
                    ))]))
                }
                TestProviderBehavior::Panic => panic!("test provider panicked"),
                TestProviderBehavior::Pending => Box::pin(futures_util::stream::pending()),
            }
        }
    }

    struct ConfiguredFactory {
        requests: Arc<Mutex<Vec<ConfiguredSubagentBuildRequest>>>,
        behavior: TestProviderBehavior,
        resource: Option<Arc<dyn SubagentResource>>,
    }

    #[async_trait]
    impl SubagentFactory for ConfiguredFactory {
        async fn build(
            &self,
            _request: SubagentBuildRequest,
        ) -> Result<Agent, SubagentFactoryError> {
            Err(SubagentFactoryError::new(
                "configured factory requires build_configured",
            ))
        }

        async fn build_configured(
            &self,
            request: ConfiguredSubagentBuildRequest,
        ) -> Result<BuiltSubagent, SubagentFactoryError> {
            mutex_lock(&self.requests).push(request.clone());
            let agent = Agent::builder(TestProvider {
                behavior: self.behavior.clone(),
            })
            .generation_config(request.effective_config.generation_config)
            .build();
            let built = BuiltSubagent::new(agent);
            Ok(match &self.resource {
                Some(resource) => built.with_resource(Arc::clone(resource)),
                None => built,
            })
        }
    }

    struct CountingResource {
        finalizations: AtomicUsize,
        dispositions: Mutex<Vec<SubagentResourceDisposition>>,
    }

    impl CountingResource {
        fn new() -> Self {
            Self {
                finalizations: AtomicUsize::new(0),
                dispositions: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SubagentResource for CountingResource {
        fn info(&self) -> SubagentResourceInfo {
            SubagentResourceInfo {
                kind: "test".to_owned(),
                location: Some("/tmp/test-subagent-resource".to_owned()),
            }
        }

        async fn finalize(
            &self,
            disposition: SubagentResourceDisposition,
        ) -> Result<SubagentResourceFinalization, SubagentFactoryError> {
            self.finalizations.fetch_add(1, Ordering::SeqCst);
            mutex_lock(&self.dispositions).push(disposition);
            Ok(SubagentResourceFinalization {
                preserved: true,
                location: Some("/tmp/test-subagent-resource".to_owned()),
                message: Some("preserved for inspection".to_owned()),
            })
        }
    }

    fn response(text: &str) -> ProviderResponse {
        ProviderResponse {
            message: AssistantMessage::text(text),
            usage: Some(TokenUsage::new(3, 2, 0)),
        }
    }

    fn pending_runtime(max_agents: usize) -> SubagentRuntime {
        SubagentRuntime::new(
            "parent-1",
            Arc::new(PendingFactory {
                entered: Arc::new(Notify::new()),
            }),
            SubagentConfig {
                max_agents,
                ..SubagentConfig::default()
            },
        )
    }

    async fn wait_for_state(runtime: &SubagentRuntime, agent_id: &str, expected: SubagentState) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime.snapshot(agent_id).unwrap().state == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("subagent did not reach {expected:?}"));
    }

    async fn wait_for_outcome(runtime: &SubagentRuntime, agent_id: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = runtime.snapshot(agent_id).unwrap();
                if snapshot.last_outcome.is_some() && snapshot.state == SubagentState::Idle {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("subagent did not finish a run");
    }

    #[tokio::test]
    async fn spawn_publishes_before_build_and_close_is_permanent_and_idempotent() {
        let runtime = pending_runtime(1);
        let mut events = runtime.subscribe();

        let spawned = tokio::time::timeout(
            Duration::from_millis(100),
            runtime.spawn(SpawnAgentRequest::new("research", "inspect the code")),
        )
        .await
        .expect("spawn must not wait for factory build")
        .unwrap();
        let spawned_event = events.recv().await.unwrap();
        assert_eq!(spawned_event.parent_id, "parent-1");
        assert_eq!(spawned_event.agent_id, spawned.agent_id);
        assert!(matches!(
            spawned_event.kind,
            SubagentEventKind::Spawned { .. }
        ));
        assert_eq!(
            runtime.snapshot(&spawned.agent_id).unwrap().state,
            SubagentState::Starting
        );

        let first_close = tokio::time::timeout(
            Duration::from_secs(1),
            runtime.close(&spawned.agent_id, "test complete"),
        )
        .await
        .expect("close while Starting must cancel build")
        .unwrap();
        assert!(!first_close.already_closed);
        assert_eq!(
            runtime.snapshot(&spawned.agent_id).unwrap().state,
            SubagentState::Closed
        );
        assert!(matches!(
            runtime.send_message(&spawned.agent_id, "too late"),
            Err(SubagentError::Closed { .. })
        ));
        assert!(matches!(
            runtime.notify(
                &spawned.agent_id,
                SubagentNotificationKind::Result,
                "too late".to_owned(),
                SubagentNotificationSource::Child,
            ),
            Err(SubagentError::Closed { .. })
        ));

        let second_close = runtime
            .close(&spawned.agent_id, "duplicate close")
            .await
            .unwrap();
        assert!(second_close.already_closed);

        let mut closed_events = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event.kind, SubagentEventKind::Closed { .. }) {
                closed_events += 1;
            }
        }
        assert_eq!(closed_events, 1);
    }

    #[tokio::test]
    async fn running_messages_reach_the_core_mailbox_safe_boundary() {
        let provider = Arc::new(ProviderState::default());
        let runtime = SubagentRuntime::new(
            "parent-2",
            Arc::new(ProviderFactory {
                state: Arc::clone(&provider),
            }),
            SubagentConfig::default(),
        );
        let mut events = runtime.subscribe();
        let spawned = runtime
            .spawn(SpawnAgentRequest::new("worker", "initial prompt"))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), provider.first_started.notified())
            .await
            .expect("first provider request did not start");
        let queued = runtime
            .send_message(&spawned.agent_id, "steer during the active stream")
            .unwrap();
        assert_eq!(queued.agent_id, spawned.agent_id);
        provider.release_first.notify_one();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let event = events.recv().await.unwrap();
                if matches!(event.kind, SubagentEventKind::RunFinished { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("subagent run did not finish");

        {
            let requests = mutex_lock(&provider.requests);
            assert_eq!(requests.len(), 2);
            let second_user_texts = requests[1]
                .messages
                .iter()
                .filter(|message| message.role == Role::User)
                .filter_map(Message::text_content)
                .collect::<Vec<_>>();
            assert!(second_user_texts.contains(&"initial prompt"));
            assert!(second_user_texts.contains(&"steer during the active stream"));
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime.snapshot(&spawned.agent_id).unwrap().state == SubagentState::Idle {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let snapshot = runtime.snapshot(&spawned.agent_id).unwrap();
        assert_eq!(snapshot.cumulative_usage, TokenUsage::new(6, 4, 0));
        assert!(matches!(
            snapshot.last_outcome,
            Some(SubagentRunOutcome::Completed { .. })
        ));
        runtime
            .close(&spawned.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_spawns_cannot_exceed_the_limit() {
        let runtime = pending_runtime(1);
        let first_runtime = runtime.clone();
        let second_runtime = runtime.clone();
        let (first, second) = tokio::join!(
            first_runtime.spawn(SpawnAgentRequest::new("one", "first")),
            second_runtime.spawn(SpawnAgentRequest::new("two", "second")),
        );
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        let error = first.err().or_else(|| second.err()).unwrap();
        assert_eq!(error, SubagentError::LimitReached { limit: 1 });
        runtime.shutdown("test complete").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cannot_miss_a_concurrent_spawn() {
        for _ in 0..32 {
            let runtime = pending_runtime(1);
            let barrier = Arc::new(Barrier::new(2));
            let spawning_runtime = runtime.clone();
            let spawning_barrier = Arc::clone(&barrier);
            let spawning = tokio::spawn(async move {
                spawning_barrier.wait().await;
                spawning_runtime
                    .spawn(SpawnAgentRequest::new("racing", "work"))
                    .await
            });
            let shutdown_runtime = runtime.clone();
            let shutdown_barrier = Arc::clone(&barrier);
            let shutting_down = tokio::spawn(async move {
                shutdown_barrier.wait().await;
                shutdown_runtime.shutdown("race test").await;
            });
            let (spawned, shutdown) = tokio::join!(spawning, shutting_down);
            shutdown.unwrap();
            let _ = spawned.unwrap();

            assert!(
                runtime
                    .snapshots()
                    .iter()
                    .all(|snapshot| snapshot.state == SubagentState::Closed)
            );
            assert_eq!(
                runtime
                    .spawn(SpawnAgentRequest::new("late", "must reject"))
                    .await,
                Err(SubagentError::ShuttingDown)
            );
        }
    }

    #[tokio::test]
    async fn notification_policy_only_suppresses_progress_wakes() {
        let runtime = pending_runtime(1);
        let mut events = runtime.subscribe();
        let spawned = runtime
            .spawn(SpawnAgentRequest::new("notify", "wait"))
            .await
            .unwrap();
        let _spawned = events.recv().await.unwrap();

        runtime
            .notify(
                &spawned.agent_id,
                SubagentNotificationKind::Progress,
                "half way".to_owned(),
                SubagentNotificationSource::Child,
            )
            .unwrap();
        let progress = events.recv().await.unwrap();
        assert!(!progress.wakes_parent());

        runtime
            .notify(
                &spawned.agent_id,
                SubagentNotificationKind::Blocker,
                "need input".to_owned(),
                SubagentNotificationSource::Child,
            )
            .unwrap();
        let blocker = events.recv().await.unwrap();
        assert!(blocker.wakes_parent());
        runtime
            .close(&spawned.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn configured_spawn_resolves_effective_config_and_validates_json() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runtime = SubagentRuntime::new(
            "parent-configured",
            Arc::new(ConfiguredFactory {
                requests: Arc::clone(&requests),
                behavior: TestProviderBehavior::Response(
                    r#"{"summary":"complete","count":1}"#.to_owned(),
                ),
                resource: None,
            }),
            SubagentConfig {
                capability_ceiling: CapabilityMode::WorkspaceEdit,
                generation_config: GenerationConfig {
                    model: Some("parent-model".to_owned()),
                    temperature: Some(0.25),
                    max_tokens: Some(512),
                    reasoning_effort: Some(crate::ReasoningEffort::Medium),
                },
                ..SubagentConfig::default()
            },
        );
        let mut events = runtime.subscribe();
        let spawned = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("inspect", "inspect the repository")
                    .agent_type(SubagentType::Explore)
                    .capability_mode(CapabilityMode::FullAccess)
                    .generation_config(GenerationConfig {
                        model: Some("child-model".to_owned()),
                        ..GenerationConfig::default()
                    })
                    .output_contract(SubagentOutputContract::Json {
                        required_fields: vec!["summary".to_owned()],
                    })
                    .isolation(SubagentIsolation::Worktree),
            )
            .await
            .unwrap();

        assert_eq!(
            spawned.effective_config.capability_mode,
            CapabilityMode::ReadOnly
        );
        assert_eq!(
            spawned.effective_config.generation_config.model.as_deref(),
            Some("child-model")
        );
        assert_eq!(
            spawned.effective_config.generation_config.reasoning_effort,
            Some(crate::ReasoningEffort::Medium)
        );
        assert_eq!(
            spawned.effective_config.generation_config.temperature,
            Some(0.25)
        );
        let event = events.recv().await.unwrap();
        assert!(matches!(
            event.kind,
            SubagentEventKind::Spawned {
                effective_config,
                ..
            } if effective_config == spawned.effective_config
        ));

        wait_for_outcome(&runtime, &spawned.agent_id).await;
        let snapshot = runtime.snapshot(&spawned.agent_id).unwrap();
        assert_eq!(snapshot.effective_config, spawned.effective_config);
        assert!(matches!(
            snapshot.validated_output,
            Some(ValidatedSubagentOutput::Json { ref value })
                if value["summary"] == "complete"
        ));
        {
            let built = mutex_lock(&requests);
            assert_eq!(built.len(), 1);
            assert_eq!(built[0].effective_config, spawned.effective_config);
        }
        runtime
            .close(&spawned.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn parent_capability_ceiling_restricts_a_general_child() {
        let runtime = SubagentRuntime::new(
            "parent-capability-ceiling",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Response("done".to_owned()),
                resource: None,
            }),
            SubagentConfig {
                capability_ceiling: CapabilityMode::WorkspaceEdit,
                ..SubagentConfig::default()
            },
        );
        let spawned = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("general", "complete")
                    .capability_mode(CapabilityMode::FullAccess),
            )
            .await
            .unwrap();
        assert_eq!(
            spawned.effective_config.capability_mode,
            CapabilityMode::WorkspaceEdit
        );
        wait_for_outcome(&runtime, &spawned.agent_id).await;
        runtime
            .close(&spawned.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn capability_ceiling_switch_closes_broader_children_and_applies_to_future_spawns() {
        let runtime = SubagentRuntime::new(
            "parent-dynamic-capability",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Response("done".to_owned()),
                resource: None,
            }),
            SubagentConfig::default(),
        );
        let first = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("full child", "complete")
                    .capability_mode(CapabilityMode::FullAccess),
            )
            .await
            .unwrap();
        assert_eq!(
            first.effective_config.capability_mode,
            CapabilityMode::FullAccess
        );

        runtime
            .close_exceeding_capability(
                CapabilityMode::WorkspaceEdit,
                "parent capability was narrowed",
            )
            .await;
        assert_eq!(
            runtime.snapshot(&first.agent_id).unwrap().state,
            SubagentState::Closed
        );
        assert_eq!(runtime.capability_ceiling(), CapabilityMode::WorkspaceEdit);

        runtime.set_capability_ceiling(CapabilityMode::FullAccess);
        let second = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("new full child", "complete")
                    .capability_mode(CapabilityMode::FullAccess),
            )
            .await
            .unwrap();
        assert_eq!(
            second.effective_config.capability_mode,
            CapabilityMode::FullAccess
        );
        runtime
            .close(&second.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn json_output_contract_failure_is_a_failed_run() {
        let runtime = SubagentRuntime::new(
            "parent-json-failure",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Response(r#"{"other":true}"#.to_owned()),
                resource: None,
            }),
            SubagentConfig::default(),
        );
        let spawned = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("json", "return json").output_contract(
                    SubagentOutputContract::Json {
                        required_fields: vec!["summary".to_owned()],
                    },
                ),
            )
            .await
            .unwrap();
        wait_for_outcome(&runtime, &spawned.agent_id).await;

        let snapshot = runtime.snapshot(&spawned.agent_id).unwrap();
        assert!(snapshot.validated_output.is_none());
        assert!(matches!(
            snapshot.last_outcome,
            Some(SubagentRunOutcome::Failed { ref error })
                if error.contains("missing required field")
        ));
        runtime
            .close(&spawned.agent_id, "test complete")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn json_output_contract_rejects_empty_required_field_names_before_spawn() {
        let runtime = pending_runtime(1);
        let result = runtime
            .spawn_configured(
                ConfiguredSpawnAgentRequest::new("json", "return json").output_contract(
                    SubagentOutputContract::Json {
                        required_fields: vec![" ".to_owned()],
                    },
                ),
            )
            .await;
        assert!(matches!(
            result,
            Err(SubagentError::InvalidConfiguration { ref message })
                if message.contains("required_fields[0]")
        ));
        assert!(runtime.snapshots().is_empty());

        assert!(
            SubagentOutputContract::Json {
                required_fields: vec!["summary".to_owned(), " summary ".to_owned()],
            }
            .normalized()
            .unwrap_err()
            .contains("duplicated")
        );
        assert_eq!(
            SubagentOutputContract::Json {
                required_fields: vec![" summary ".to_owned()],
            }
            .normalized()
            .unwrap(),
            SubagentOutputContract::Json {
                required_fields: vec!["summary".to_owned()],
            }
        );
    }

    #[tokio::test]
    async fn resource_finalizer_runs_once_across_close_and_shutdown() {
        let resource = Arc::new(CountingResource::new());
        let runtime = SubagentRuntime::new(
            "parent-resource",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Response("done".to_owned()),
                resource: Some(resource.clone()),
            }),
            SubagentConfig::default(),
        );
        let spawned = runtime
            .spawn(SpawnAgentRequest::new("resource", "complete"))
            .await
            .unwrap();
        wait_for_outcome(&runtime, &spawned.agent_id).await;
        let mut events = runtime.subscribe();
        runtime
            .close(&spawned.agent_id, "first close")
            .await
            .unwrap();
        runtime
            .close(&spawned.agent_id, "second close")
            .await
            .unwrap();
        runtime.shutdown("shutdown after close").await;

        assert_eq!(resource.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(
            mutex_lock(&resource.dispositions).as_slice(),
            &[SubagentResourceDisposition::Closed]
        );
        assert!(
            runtime
                .snapshot(&spawned.agent_id)
                .unwrap()
                .resource_finalization
                .is_some()
        );
        let closed_reason = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let SubagentEventKind::Closed { reason, .. } = events.recv().await.unwrap().kind
                {
                    break reason;
                }
            }
        })
        .await
        .unwrap();
        assert!(closed_reason.contains("/tmp/test-subagent-resource"));
    }

    #[tokio::test]
    async fn resource_finalizer_runs_once_after_worker_panic() {
        let resource = Arc::new(CountingResource::new());
        let runtime = SubagentRuntime::new(
            "parent-panic-resource",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Panic,
                resource: Some(resource.clone()),
            }),
            SubagentConfig::default(),
        );
        let spawned = runtime
            .spawn(SpawnAgentRequest::new("panic", "trigger panic"))
            .await
            .unwrap();
        wait_for_state(&runtime, &spawned.agent_id, SubagentState::Closed).await;

        assert_eq!(resource.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(
            mutex_lock(&resource.dispositions).as_slice(),
            &[SubagentResourceDisposition::RuntimeFailed]
        );
    }

    #[tokio::test]
    async fn shutdown_finalizes_a_running_resource_once() {
        let resource = Arc::new(CountingResource::new());
        let runtime = SubagentRuntime::new(
            "parent-shutdown-resource",
            Arc::new(ConfiguredFactory {
                requests: Arc::new(Mutex::new(Vec::new())),
                behavior: TestProviderBehavior::Pending,
                resource: Some(resource.clone()),
            }),
            SubagentConfig::default(),
        );
        let spawned = runtime
            .spawn(SpawnAgentRequest::new("pending", "wait forever"))
            .await
            .unwrap();
        wait_for_state(&runtime, &spawned.agent_id, SubagentState::Running).await;
        tokio::time::timeout(Duration::from_secs(1), runtime.shutdown("test shutdown"))
            .await
            .expect("shutdown must stop the child");

        assert_eq!(resource.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(
            mutex_lock(&resource.dispositions).as_slice(),
            &[SubagentResourceDisposition::Closed]
        );
    }
}
