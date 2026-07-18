use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::FutureExt;
use phi::{
    Agent, AgentEvent, AgentRunControl, AgentRunOutcome, AssistantDelta, CapabilityMode, Content,
    ContextCompactionRunOutcome, ContextUsage, DEFAULT_CONTEXT_COMPACTION_BOUNDARY_MESSAGE,
    Message, MessageVisibility, ReasoningEffort, Role, SessionSnapshot, SessionStorage,
    SkillCatalog, SkillDiagnostic, SkillInvocation, SkillMetadata, SubagentEvent,
    SubagentEventKind, SubagentNotificationKind, SubagentRuntime, SubagentSnapshot, SubagentState,
    TokenUsage, Workspace,
};
use thiserror::Error;
use tokio::sync::{
    OwnedSemaphorePermit, Semaphore, TryAcquireError, broadcast, mpsc, oneshot, watch,
};

use super::{
    AskUserId, PinnedAgentProfile, RunId, SessionId,
    ask_user::{
        AskUserAnswer, AskUserRequest, AskUserTool, PendingAskUserRequest, validate_answers,
    },
    compile_agent_profile, default_agent_profile,
};
use crate::store::{ControlStore, SessionRecord};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStatus {
    AwaitingFirstPrompt,
    Idle,
    Compacting,
    Running,
    Stopping,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextCompactionPhase {
    Started,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextCompactionView {
    pub phase: ContextCompactionPhase,
    /// Transcript position where the client should render the status divider.
    pub history_index: usize,
    /// Replacement range hidden from public history snapshots.
    pub hidden_range: Option<std::ops::Range<usize>>,
    pub after_message_count: Option<usize>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssistantDraft {
    pub reasoning: String,
    pub text: String,
    pub tool_calls: Vec<ToolCallDraft>,
    /// Durable assistant message index that can be forked before this draft's
    /// tool calls. It is exposed only after the tool-call journal is saved.
    pub fork_message_index: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallDraft {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentView {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub agent_profile_revision: u64,
    pub initialized: bool,
    pub status: AgentStatus,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub capability_mode: CapabilityMode,
    pub model: String,
    pub workspace: Option<Workspace>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub config_revision: u64,
    /// Complete durable conversation projection; never used as Provider input.
    pub display_messages: Vec<Message>,
    pub messages: Vec<Message>,
    pub context_compactions: Vec<ContextCompactionView>,
    pub context_compaction: Option<ContextCompactionView>,
    pub draft: Option<AssistantDraft>,
    pub last_usage: Option<TokenUsage>,
    pub context_usage: Option<ContextUsage>,
    pub cumulative_usage: TokenUsage,
    pub pending_asks: Vec<AskUserRequest>,
    pub subagents: Vec<SubagentSummary>,
    pub last_event_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentSummary {
    pub agent_id: String,
    pub description: String,
    pub state: SubagentState,
    pub last_sequence: u64,
}

/// Lightweight control-plane projection. It intentionally excludes transcript
/// and provider state so status/config checks do not clone a conversation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSummary {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub profile_id: String,
    pub agent_profile_id: String,
    pub agent_profile_revision: u64,
    pub initialized: bool,
    pub status: AgentStatus,
    pub active_run_id: Option<RunId>,
    pub queued_runs: usize,
    pub capability_mode: CapabilityMode,
    pub model: String,
    pub workspace: Option<Workspace>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub config_revision: u64,
    pub message_count: usize,
    pub subagents: Vec<SubagentSummary>,
    pub last_event_sequence: u64,
}

#[derive(Clone, Debug)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub kind: RuntimeEventKind,
}

#[derive(Clone, Debug)]
pub enum RuntimeEventKind {
    StateChanged {
        status: AgentStatus,
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
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
        revision: u64,
    },
    CapabilityModeChanged {
        capability_mode: CapabilityMode,
    },
    AskUserRequested {
        request: AskUserRequest,
    },
    AskUserAnswered {
        ask_id: AskUserId,
    },
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
    SubagentsResynced {
        subagents: Vec<SubagentSummary>,
    },
    Subagent(SubagentEvent),
    Agent(AgentEvent),
}

#[derive(Clone)]
pub struct AgentHandle {
    session_id: SessionId,
    commands: mpsc::Sender<AgentCommand>,
    events: broadcast::Sender<RuntimeEvent>,
    state: watch::Receiver<AgentView>,
    hub: Arc<EventHub>,
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    active_compaction: Arc<Mutex<Option<AgentRunControl>>>,
    prompt_slots: Arc<Semaphore>,
    compaction_slot: Arc<Semaphore>,
    skills: SkillCatalog,
    subagents: Option<SubagentRuntime>,
    agent_profile: PinnedAgentProfile,
}

impl AgentHandle {
    pub fn spawn(
        session_id: SessionId,
        agent: Agent,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self::spawn_with_skills(
            session_id,
            agent,
            profile_id,
            model,
            reasoning_effort,
            SkillCatalog::default(),
        )
    }

    pub fn spawn_with_skills(
        session_id: SessionId,
        agent: Agent,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
        skills: SkillCatalog,
    ) -> Self {
        Self::spawn_with_skills_and_subagents(
            session_id,
            agent,
            profile_id,
            model,
            reasoning_effort,
            skills,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_skills_and_subagents(
        session_id: SessionId,
        agent: Agent,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
        skills: SkillCatalog,
        subagents: Option<SubagentRuntime>,
    ) -> Self {
        let workspace = agent
            .workspace()
            .cloned()
            .unwrap_or_else(|| Workspace::new("."));
        let agent_profile = compile_agent_profile(&default_agent_profile(), &workspace)
            .expect("the built-in Agent Profile must compile");
        Self::spawn_configured_with_skills_and_subagents(
            session_id,
            agent,
            profile_id,
            agent_profile,
            model,
            reasoning_effort,
            skills,
            subagents,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_configured_with_skills_and_subagents(
        session_id: SessionId,
        mut agent: Agent,
        profile_id: impl Into<String>,
        agent_profile: PinnedAgentProfile,
        model: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
        skills: SkillCatalog,
        subagents: Option<SubagentRuntime>,
    ) -> Self {
        let stored_agent_profile = agent_profile.clone();
        let (ask_user_tool, ask_user_requests) = AskUserTool::channel();
        agent.add_mandatory_tool(ask_user_tool);
        // Subscribe before taking the initial projection so delegation that
        // races construction is either represented by the snapshot, buffered
        // as an event, or both (the projection update is idempotent).
        let subagent_events = subagents.as_ref().map(SubagentRuntime::subscribe);
        let initial_messages = agent.messages().to_vec();
        let initial_display_messages = agent.session_history().messages.clone();
        let initial_context_compactions = restored_context_compactions(&agent);
        let initial_context_compaction = initial_context_compactions
            .last()
            .cloned()
            .or_else(|| restored_context_compaction(&initial_messages));
        let initial = AgentView {
            session_id,
            title: None,
            profile_id: profile_id.into(),
            agent_profile_id: agent_profile.agent_profile_id,
            agent_profile_revision: agent_profile.revision,
            initialized: false,
            status: AgentStatus::AwaitingFirstPrompt,
            active_run_id: None,
            queued_runs: 0,
            capability_mode: agent.capability_mode(),
            model: model.into(),
            workspace: agent.workspace().cloned(),
            reasoning_effort,
            config_revision: 0,
            display_messages: initial_display_messages,
            messages: initial_messages,
            context_compactions: initial_context_compactions,
            context_compaction: initial_context_compaction,
            draft: None,
            last_usage: agent.last_usage(),
            context_usage: agent.context_usage(),
            cumulative_usage: agent.cumulative_usage(),
            pending_asks: Vec::new(),
            subagents: subagents
                .as_ref()
                .map(SubagentRuntime::snapshots)
                .unwrap_or_default()
                .iter()
                .map(subagent_summary)
                .collect(),
            last_event_sequence: 0,
        };
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (state_sender, state) = watch::channel(initial);
        let hub = Arc::new(EventHub {
            session_id,
            publish_lock: Mutex::new(()),
            queued_run_ids: Mutex::new(VecDeque::new()),
            sequence: AtomicU64::new(0),
            events: events.clone(),
            state: state_sender,
        });
        let active_run = Arc::new(Mutex::new(None));
        let active_compaction = Arc::new(Mutex::new(None));
        let prompt_slots = Arc::new(Semaphore::new(COMMAND_CAPACITY));
        let compaction_slot = Arc::new(Semaphore::new(1));

        let listener_hub = Arc::clone(&hub);
        agent.subscribe(move |event| listener_hub.publish_agent(event));

        let actor_hub = Arc::clone(&hub);
        let actor_active_run = Arc::clone(&active_run);
        let actor_active_compaction = Arc::clone(&active_compaction);
        let supervisor_hub = Arc::clone(&hub);
        let supervisor_active_run = Arc::clone(&active_run);
        let supervisor_active_compaction = Arc::clone(&active_compaction);
        let supervisor_prompt_slots = Arc::clone(&prompt_slots);
        let supervisor_compaction_slot = Arc::clone(&compaction_slot);
        let supervisor_subagents = subagents.clone();
        let actor_prompt_slots = Arc::clone(&prompt_slots);
        let actor_subagents = subagents.clone();
        tokio::spawn(async move {
            let outcome = AssertUnwindSafe(run_actor(
                agent,
                ActorRuntime {
                    commands: command_receiver,
                    ask_user_requests,
                    hub: actor_hub,
                    active_run: actor_active_run,
                    active_compaction: actor_active_compaction,
                    prompt_slots: actor_prompt_slots,
                    subagents: actor_subagents,
                    subagent_events,
                },
            ))
            .catch_unwind()
            .await;
            if let Err(payload) = outcome {
                let message = panic_message(payload);
                supervisor_prompt_slots.close();
                supervisor_compaction_slot.close();
                if let Some(subagents) = supervisor_subagents {
                    subagents.shutdown("parent agent actor crashed").await;
                }
                supervisor_hub.actor_crashed(message.clone());
                let active = supervisor_active_run
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(active) = active {
                    active.control.stop();
                    supervisor_hub.run_failed(active.id, message.clone(), AgentStatus::Closing);
                }
                if let Some(control) = supervisor_active_compaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    control.stop();
                }
                supervisor_hub.fail_all_queued(&message);
                supervisor_hub.closed();
            }
        });

        Self {
            session_id,
            commands,
            events,
            state,
            hub,
            active_run,
            active_compaction,
            prompt_slots,
            compaction_slot,
            skills,
            subagents,
            agent_profile: stored_agent_profile,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn skills(&self) -> &[SkillMetadata] {
        self.skills.skills()
    }

    pub fn skill_diagnostics(&self) -> &[SkillDiagnostic] {
        self.skills.diagnostics()
    }

    pub fn skill_catalog(&self) -> &SkillCatalog {
        &self.skills
    }

    pub fn subagents(&self) -> Option<&SubagentRuntime> {
        self.subagents.as_ref()
    }

    pub fn agent_profile(&self) -> &PinnedAgentProfile {
        &self.agent_profile
    }

    pub fn prepare_prompt(
        &self,
        content: Content,
        skill: Option<&SkillInvocation>,
    ) -> Result<Content, AgentHandleError> {
        match skill {
            Some(skill) => self
                .skills
                .apply_to_prompt(skill, content)
                .map_err(|error| AgentHandleError::InvalidCommand {
                    message: error.to_string(),
                }),
            None => Ok(content),
        }
    }

    pub fn snapshot(&self) -> AgentView {
        self.state.borrow().clone()
    }

    /// Returns the current status without cloning the transcript held by the
    /// watch projection.
    pub fn status(&self) -> AgentStatus {
        self.state.borrow().status
    }

    /// Returns whether persistent session state has been attached without
    /// cloning the transcript held by the watch projection.
    pub fn is_initialized(&self) -> bool {
        self.state.borrow().initialized
    }

    pub fn summary(&self) -> AgentSummary {
        let state = self.state.borrow();
        AgentSummary {
            session_id: state.session_id,
            title: state.title.clone(),
            profile_id: state.profile_id.clone(),
            agent_profile_id: state.agent_profile_id.clone(),
            agent_profile_revision: state.agent_profile_revision,
            initialized: state.initialized,
            status: state.status,
            active_run_id: state.active_run_id,
            queued_runs: state.queued_runs,
            capability_mode: state.capability_mode,
            model: state.model.clone(),
            workspace: state.workspace.clone(),
            reasoning_effort: state.reasoning_effort,
            config_revision: state.config_revision,
            message_count: state.messages.len(),
            subagents: state.subagents.clone(),
            last_event_sequence: state.last_event_sequence,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    pub async fn initialize(
        &self,
        record: SessionRecord,
        session_storage: Arc<dyn SessionStorage>,
        control_store: Arc<dyn ControlStore>,
    ) -> Result<(), AgentHandleError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::Initialize {
                record: Box::new(record),
                session_storage,
                control_store,
                reply,
            })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    pub async fn enqueue_prompt(&self, content: Content) -> Result<QueuedRun, AgentHandleError> {
        if matches!(self.status(), AgentStatus::Closing | AgentStatus::Closed) {
            return Err(self.stopped_error());
        }

        let queue_permit = Arc::clone(&self.prompt_slots)
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => AgentHandleError::QueueFull {
                    session_id: self.session_id,
                    capacity: COMMAND_CAPACITY,
                },
                TryAcquireError::Closed => self.stopped_error(),
            })?;
        let run_id = RunId::new();
        let (admission, response) = oneshot::channel();
        self.commands
            .try_send(AgentCommand::Prompt {
                run_id,
                content,
                visibility: MessageVisibility::Public,
                queue_permit,
                admission: Some(admission),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AgentHandleError::QueueFull {
                    session_id: self.session_id,
                    capacity: COMMAND_CAPACITY,
                },
                mpsc::error::TrySendError::Closed(_) => self.stopped_error(),
            })?;
        let position = response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|()| self.stopped_error())?;
        Ok(QueuedRun { run_id, position })
    }

    /// Admits one explicit context compaction and returns as soon as the actor
    /// has transitioned to `Compacting`. Completion or failure is reported by
    /// the corresponding ordered Agent event.
    pub async fn compact_context(
        &self,
        instructions: Option<String>,
    ) -> Result<(), AgentHandleError> {
        let status = self.status();
        if status != AgentStatus::Idle {
            return Err(AgentHandleError::Busy {
                session_id: self.session_id,
                status,
            });
        }
        let compaction_permit = Arc::clone(&self.compaction_slot)
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => AgentHandleError::Busy {
                    session_id: self.session_id,
                    status: AgentStatus::Compacting,
                },
                TryAcquireError::Closed => self.stopped_error(),
            })?;
        let status = self.status();
        if status != AgentStatus::Idle {
            return Err(AgentHandleError::Busy {
                session_id: self.session_id,
                status,
            });
        }

        let (admission, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::CompactContext {
                instructions,
                compaction_permit,
                admission,
            })
            .await
            .map_err(|_| self.stopped_error())?;
        response.await.map_err(|_| self.response_error())?
    }

    pub async fn set_model(&self, model: String) -> Result<(), AgentHandleError> {
        self.ensure_configurable()?;
        if model.trim().is_empty() {
            return Err(AgentHandleError::InvalidCommand {
                message: "model must not be empty".to_owned(),
            });
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SetModel { model, reply })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    pub async fn set_title(&self, title: String) -> Result<(), AgentHandleError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SetTitle { title, reply })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    pub async fn set_pinned(&self, pinned: bool) -> Result<(), AgentHandleError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SetPinned { pinned, reply })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    /// Returns a protocol-complete in-memory snapshot at an actor boundary.
    ///
    /// Forking is rejected while a run or compaction is active so callers can
    /// never clone a streaming draft or race a transcript replacement.
    pub async fn snapshot_for_fork(&self) -> Result<SessionSnapshot, AgentHandleError> {
        let _fork_guard = self.acquire_fork_guard()?;
        let status = self.status();
        if status != AgentStatus::Idle {
            return Err(AgentHandleError::Busy {
                session_id: self.session_id,
                status,
            });
        }

        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SnapshotForFork { reply })
            .await
            .map_err(|_| self.stopped_error())?;
        response.await.map_err(|_| self.response_error())?
    }

    /// Prevents context replacement while a durable fork checkpoint is read.
    /// Running turns may continue appending protocol-complete checkpoints.
    pub(crate) fn acquire_fork_guard(&self) -> Result<OwnedSemaphorePermit, AgentHandleError> {
        Arc::clone(&self.compaction_slot)
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => AgentHandleError::Busy {
                    session_id: self.session_id,
                    status: AgentStatus::Compacting,
                },
                TryAcquireError::Closed => self.stopped_error(),
            })
    }

    pub async fn set_reasoning_effort(
        &self,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<(), AgentHandleError> {
        self.ensure_configurable()?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SetReasoning {
                reasoning_effort,
                reply,
            })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    pub async fn set_capability_mode(
        &self,
        capability_mode: CapabilityMode,
    ) -> Result<(), AgentHandleError> {
        self.ensure_configurable()?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::SetCapabilityMode {
                capability_mode,
                reply,
            })
            .await
            .map_err(|_| self.stopped_error())?;
        response
            .await
            .map_err(|_| self.response_error())?
            .map_err(|message| AgentHandleError::Operation {
                session_id: self.session_id,
                message,
            })
    }

    pub async fn answer_ask_user(
        &self,
        ask_id: AskUserId,
        answers: Vec<AskUserAnswer>,
    ) -> Result<(), AgentHandleError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::AnswerAskUser {
                ask_id,
                answers,
                reply,
            })
            .await
            .map_err(|_| self.stopped_error())?;
        response.await.map_err(|_| self.response_error())?
    }

    pub fn stop(&self, run_id: RunId) -> Result<(), AgentHandleError> {
        let mut active = self.active_run.lock().expect("active run lock poisoned");
        let status = self.hub.current_status();
        if matches!(status, AgentStatus::Closing | AgentStatus::Closed) {
            return Err(AgentHandleError::Busy {
                session_id: self.session_id,
                status,
            });
        }
        let Some(active) = active.as_mut() else {
            return Err(AgentHandleError::NoActiveRun {
                session_id: self.session_id,
            });
        };
        if active.id != run_id {
            return Err(AgentHandleError::RunMismatch {
                session_id: self.session_id,
                active: active.id,
                requested: run_id,
            });
        }
        active.request_stop();
        self.hub.status(AgentStatus::Stopping);
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), AgentHandleError> {
        if self.status() == AgentStatus::Closed {
            if let Some(subagents) = &self.subagents {
                subagents.shutdown("parent agent is shut down").await;
            }
            return Ok(());
        }
        {
            // Share the same linearization lock as run start/completion and
            // stop. Whichever side wins leaves a coherent state; a terminal
            // run can no longer overwrite Closing with Idle.
            let mut active = self.active_run.lock().expect("active run lock poisoned");
            self.hub.status(AgentStatus::Closing);
            self.prompt_slots.close();
            self.compaction_slot.close();
            if let Some(active) = active.as_mut() {
                active.request_stop();
            }
        }
        if let Some(control) = self
            .active_compaction
            .lock()
            .expect("active compaction lock poisoned")
            .as_ref()
        {
            control.stop();
        }

        if let Some(subagents) = &self.subagents {
            subagents.shutdown("parent agent is shutting down").await;
        }

        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::Shutdown { reply })
            .await
            .map_err(|_| self.stopped_error())?;
        response.await.map_err(|_| self.response_error())
    }

    fn ensure_configurable(&self) -> Result<(), AgentHandleError> {
        let status = if self.compaction_slot.available_permits() == 0 {
            AgentStatus::Compacting
        } else {
            self.status()
        };
        if matches!(status, AgentStatus::AwaitingFirstPrompt | AgentStatus::Idle) {
            Ok(())
        } else {
            Err(AgentHandleError::Busy {
                session_id: self.session_id,
                status,
            })
        }
    }

    fn stopped_error(&self) -> AgentHandleError {
        AgentHandleError::ActorStopped {
            session_id: self.session_id,
        }
    }

    fn response_error(&self) -> AgentHandleError {
        AgentHandleError::ResponseDropped {
            session_id: self.session_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuedRun {
    pub run_id: RunId,
    pub position: usize,
}

#[derive(Clone)]
struct ActiveRun {
    id: RunId,
    control: AgentRunControl,
    stop_requested: bool,
}

impl ActiveRun {
    fn request_stop(&mut self) {
        // This flag and terminal publication are guarded by the same mutex.
        // Therefore an accepted stop either wins before terminalization (and
        // a concurrently ready `Completed` outcome is exposed as stopped), or
        // loses after the actor has removed the run and is rejected as late.
        self.stop_requested = true;
        self.control.stop();
    }
}

struct MetadataBinding {
    record: SessionRecord,
    store: Arc<dyn ControlStore>,
}

enum AgentCommand {
    Initialize {
        record: Box<SessionRecord>,
        session_storage: Arc<dyn SessionStorage>,
        control_store: Arc<dyn ControlStore>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Prompt {
        run_id: RunId,
        content: Content,
        visibility: MessageVisibility,
        queue_permit: OwnedSemaphorePermit,
        admission: Option<oneshot::Sender<Result<usize, ()>>>,
    },
    CompactContext {
        instructions: Option<String>,
        compaction_permit: OwnedSemaphorePermit,
        admission: oneshot::Sender<Result<(), AgentHandleError>>,
    },
    SetModel {
        model: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetTitle {
        title: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetPinned {
        pinned: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SnapshotForFork {
        reply: oneshot::Sender<Result<SessionSnapshot, AgentHandleError>>,
    },
    SetReasoning {
        reasoning_effort: Option<ReasoningEffort>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetCapabilityMode {
        capability_mode: CapabilityMode,
        reply: oneshot::Sender<Result<(), String>>,
    },
    AnswerAskUser {
        ask_id: AskUserId,
        answers: Vec<AskUserAnswer>,
        reply: oneshot::Sender<Result<(), AgentHandleError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

struct ActorRuntime {
    commands: mpsc::Receiver<AgentCommand>,
    ask_user_requests: mpsc::UnboundedReceiver<PendingAskUserRequest>,
    hub: Arc<EventHub>,
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    active_compaction: Arc<Mutex<Option<AgentRunControl>>>,
    prompt_slots: Arc<Semaphore>,
    subagents: Option<SubagentRuntime>,
    subagent_events: Option<broadcast::Receiver<SubagentEvent>>,
}

async fn run_actor(mut agent: Agent, runtime: ActorRuntime) {
    let ActorRuntime {
        mut commands,
        mut ask_user_requests,
        hub,
        active_run,
        active_compaction,
        prompt_slots,
        subagents,
        mut subagent_events,
    } = runtime;
    let mut backlog = VecDeque::new();
    let mut binding: Option<MetadataBinding> = None;
    let mut shutdown_reply = None;
    let mut closing = false;

    loop {
        let command = match backlog.pop_front() {
            Some(command) => Some(command),
            None => loop {
                tokio::select! {
                    command = commands.recv() => break command,
                    event = receive_subagent_event(&mut subagent_events) => {
                        match event {
                            Ok(event) => handle_subagent_event(
                                event,
                                &hub,
                                &prompt_slots,
                                &mut backlog,
                            ),
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                resync_subagents(&hub, subagents.as_ref(), skipped);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                subagent_events = None;
                            }
                        }
                        if let Some(command) = backlog.pop_front() {
                            break Some(command);
                        }
                    }
                }
            },
        };
        let Some(mut command) = command else {
            closing = true;
            break;
        };
        if !admit_prompt(&mut command, &hub) {
            continue;
        }

        match command {
            AgentCommand::Initialize {
                record,
                session_storage,
                control_store,
                reply,
            } => {
                let record = *record;
                if hub.is_initialized() {
                    let _ = reply.send(Err("session is already initialized".to_owned()));
                    continue;
                }
                agent.set_model(record.model.clone());
                agent.set_reasoning_effort(record.reasoning_effort);
                match agent
                    .attach_session(record.id.to_string(), session_storage)
                    .await
                {
                    Ok(()) => {
                        hub.initialized(&agent, &record);
                        binding = Some(MetadataBinding {
                            record,
                            store: control_store,
                        });
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error.to_string()));
                    }
                }
            }
            AgentCommand::Prompt {
                run_id,
                content,
                visibility,
                queue_permit,
                admission: _,
            } => {
                hub.run_dequeued(run_id);
                // Capacity accounts for waiting prompts only. Once this run
                // becomes active, release its admission slot.
                drop(queue_permit);
                if closing || matches!(hub.current_status(), AgentStatus::Closing) {
                    hub.run_failed(run_id, "agent is closing".to_owned(), AgentStatus::Closing);
                    continue;
                }
                if !hub.is_initialized() {
                    hub.run_failed(
                        run_id,
                        "session is not initialized".to_owned(),
                        AgentStatus::AwaitingFirstPrompt,
                    );
                    continue;
                }

                let control = AgentRunControl::new();
                {
                    let mut active = active_run.lock().expect("active run lock poisoned");
                    *active = Some(ActiveRun {
                        id: run_id,
                        control: control.clone(),
                        stop_requested: false,
                    });
                    hub.run_started(run_id);
                }

                let mut pending_asks = HashMap::new();
                let mut run = Box::pin(agent.prompt_content_with_visibility_controlled(
                    content,
                    visibility,
                    control.clone(),
                ));
                let result = loop {
                    tokio::select! {
                        biased;
                        request = ask_user_requests.recv() => {
                            let Some(request) = request else {
                                closing = true;
                                control.stop();
                                hub.operation_failed(
                                    "askuser",
                                    "askuser request channel closed unexpectedly".to_owned(),
                                );
                                hub.status(AgentStatus::Closing);
                                break run.as_mut().await;
                            };
                            register_pending_ask(&mut pending_asks, &hub, request);
                        }
                        // Delegation is published synchronously by the core
                        // runtime. If it and the parent run become ready in
                        // the same poll, expose the child before publishing
                        // the parent's terminal event.
                        event = receive_subagent_event(&mut subagent_events) => {
                            match event {
                                Ok(event) => handle_subagent_event(
                                    event,
                                    &hub,
                                    &prompt_slots,
                                    &mut backlog,
                                ),
                                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                    resync_subagents(&hub, subagents.as_ref(), skipped);
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    subagent_events = None;
                                }
                            }
                        }
                        result = &mut run => break result,
                        command = commands.recv() => {
                            let Some(mut command) = command else {
                                closing = true;
                                control.stop();
                                hub.status(AgentStatus::Closing);
                                break run.as_mut().await;
                            };
                            if !admit_prompt(&mut command, &hub) {
                                continue;
                            }
                            match command {
                                AgentCommand::Shutdown { reply } => {
                                    shutdown_reply = Some(reply);
                                    closing = true;
                                    control.stop();
                                    hub.status(AgentStatus::Closing);
                                    break run.as_mut().await;
                                }
                                AgentCommand::SetModel { reply, .. } => {
                                    let _ = reply.send(Err("cannot change model while the agent is running".to_owned()));
                                }
                                AgentCommand::SetReasoning { reply, .. } => {
                                    let _ = reply.send(Err("cannot change reasoning effort while the agent is running".to_owned()));
                                }
                                AgentCommand::SetCapabilityMode { reply, .. } => {
                                    let _ = reply.send(Err("cannot change capability mode while the agent is running".to_owned()));
                                }
                                AgentCommand::CompactContext { admission, .. } => {
                                    let _ = admission.send(Err(AgentHandleError::Busy {
                                        session_id: hub.session_id,
                                        status: hub.current_status(),
                                    }));
                                }
                                AgentCommand::SetTitle { title, reply } => {
                                    let result = apply_title(&hub, &mut binding, title).await;
                                    if let Err(error) = &result {
                                        hub.operation_failed("set_title", error.clone());
                                    }
                                    let _ = reply.send(result);
                                }
                                AgentCommand::SetPinned { pinned, reply } => {
                                    let result = apply_pinned(&mut binding, pinned).await;
                                    if let Err(error) = &result {
                                        hub.operation_failed("set_pinned", error.clone());
                                    }
                                    let _ = reply.send(result);
                                }
                                AgentCommand::SnapshotForFork { reply } => {
                                    let _ = reply.send(Err(AgentHandleError::Busy {
                                        session_id: hub.session_id,
                                        status: hub.current_status(),
                                    }));
                                }
                                AgentCommand::AnswerAskUser {
                                    ask_id,
                                    answers,
                                    reply,
                                } => {
                                    let active = active_run.lock().expect("active run lock poisoned");
                                    let stopping = active.as_ref().is_none_or(|active| {
                                        active.stop_requested || active.control.is_stopped()
                                    }) || run_is_stopping(&control, &hub);
                                    let result = if stopping {
                                        cancel_pending_ask(
                                            &mut pending_asks,
                                            &hub,
                                            ask_id,
                                            "askuser request was cancelled because the run is stopping",
                                        );
                                        Err(AgentHandleError::AskUserNotPending {
                                            session_id: hub.session_id,
                                            ask_id,
                                        })
                                    } else {
                                        answer_pending_ask(
                                            &mut pending_asks,
                                            &hub,
                                            ask_id,
                                            answers,
                                        )
                                    };
                                    drop(active);
                                    let _ = reply.send(result);
                                }
                                command => backlog.push_back(command),
                            }
                        }
                    }
                };
                drop(run);
                cancel_pending_asks(
                    &mut pending_asks,
                    &hub,
                    "askuser request was cancelled because the run ended",
                );
                drain_unregistered_asks(
                    &mut ask_user_requests,
                    "askuser request was cancelled because the run ended",
                );
                {
                    let mut active = active_run.lock().expect("active run lock poisoned");
                    let stop_requested = active
                        .as_ref()
                        .is_some_and(|active| active.id == run_id && active.stop_requested);
                    let terminal_status = if closing || hub.current_status() == AgentStatus::Closing
                    {
                        AgentStatus::Closing
                    } else {
                        AgentStatus::Idle
                    };
                    match result {
                        Ok(AgentRunOutcome::Completed(_)) if stop_requested => {
                            hub.run_stopped(run_id, terminal_status)
                        }
                        Ok(AgentRunOutcome::Completed(_)) => {
                            hub.run_completed(run_id, terminal_status)
                        }
                        Ok(AgentRunOutcome::Stopped) => hub.run_stopped(run_id, terminal_status),
                        Err(error) => hub.run_failed(run_id, error.to_string(), terminal_status),
                    }
                    *active = None;
                }
                if closing {
                    break;
                }
            }
            AgentCommand::CompactContext {
                instructions,
                compaction_permit,
                admission,
            } => {
                if agent.context_compactor_name().is_none() || agent.messages().is_empty() {
                    let _ = admission.send(Err(AgentHandleError::InvalidCommand {
                        message: "the session has no context available to compact".to_owned(),
                    }));
                    drop(compaction_permit);
                    continue;
                }

                let control = AgentRunControl::new();
                {
                    // Use the same lifecycle lock as run start/completion and
                    // shutdown so Closing can never be overwritten by a racing
                    // compaction admission.
                    let _lifecycle = active_run.lock().expect("active run lock poisoned");
                    let status = hub.current_status();
                    if status != AgentStatus::Idle {
                        let _ = admission.send(Err(AgentHandleError::Busy {
                            session_id: hub.session_id,
                            status,
                        }));
                        drop(compaction_permit);
                        continue;
                    }
                    *active_compaction
                        .lock()
                        .expect("active compaction lock poisoned") = Some(control.clone());
                    hub.status(AgentStatus::Compacting);
                }
                if admission.send(Ok(())).is_err() {
                    let _lifecycle = active_run.lock().expect("active run lock poisoned");
                    active_compaction
                        .lock()
                        .expect("active compaction lock poisoned")
                        .take();
                    if hub.current_status() != AgentStatus::Closing {
                        hub.status(AgentStatus::Idle);
                    }
                    drop(compaction_permit);
                    continue;
                }

                let result = agent
                    .compact_context_controlled(instructions, control)
                    .await;
                {
                    let _lifecycle = active_run.lock().expect("active run lock poisoned");
                    active_compaction
                        .lock()
                        .expect("active compaction lock poisoned")
                        .take();
                    if !closing && hub.current_status() != AgentStatus::Closing {
                        hub.status(AgentStatus::Idle);
                    }
                }
                match result {
                    Ok(
                        ContextCompactionRunOutcome::Completed(_)
                        | ContextCompactionRunOutcome::Stopped,
                    ) => {}
                    Err(error) => {
                        hub.operation_failed("compact_context", error.to_string());
                    }
                }
                drop(compaction_permit);
            }
            AgentCommand::SetModel { model, reply } => {
                let result = apply_model(&mut agent, &hub, &mut binding, model).await;
                if let Err(error) = &result {
                    hub.operation_failed("set_model", error.clone());
                }
                let _ = reply.send(result);
            }
            AgentCommand::SetTitle { title, reply } => {
                let result = apply_title(&hub, &mut binding, title).await;
                if let Err(error) = &result {
                    hub.operation_failed("set_title", error.clone());
                }
                let _ = reply.send(result);
            }
            AgentCommand::SetPinned { pinned, reply } => {
                let result = apply_pinned(&mut binding, pinned).await;
                if let Err(error) = &result {
                    hub.operation_failed("set_pinned", error.clone());
                }
                let _ = reply.send(result);
            }
            AgentCommand::SnapshotForFork { reply } => {
                let status = hub.current_status();
                let result = if status == AgentStatus::Idle {
                    Ok(SessionSnapshot {
                        id: hub.session_id.to_string(),
                        workspace: agent.workspace().cloned(),
                        messages: agent.messages().to_vec(),
                        history: phi::SessionHistory::from_messages(agent.messages()),
                        last_usage: agent.last_usage(),
                        cumulative_usage: agent.cumulative_usage(),
                        capability_mode: agent.capability_mode(),
                    })
                } else {
                    Err(AgentHandleError::Busy {
                        session_id: hub.session_id,
                        status,
                    })
                };
                let _ = reply.send(result);
            }
            AgentCommand::SetReasoning {
                reasoning_effort,
                reply,
            } => {
                let result =
                    apply_reasoning(&mut agent, &hub, &mut binding, reasoning_effort).await;
                if let Err(error) = &result {
                    hub.operation_failed("set_reasoning_effort", error.clone());
                }
                let _ = reply.send(result);
            }
            AgentCommand::SetCapabilityMode {
                capability_mode,
                reply,
            } => {
                let result =
                    apply_capability_mode(&mut agent, &hub, subagents.as_ref(), capability_mode)
                        .await;
                if let Err(error) = &result {
                    hub.operation_failed("set_capability_mode", error.clone());
                }
                let _ = reply.send(result);
            }
            AgentCommand::AnswerAskUser { ask_id, reply, .. } => {
                let _ = reply.send(Err(AgentHandleError::AskUserNotPending {
                    session_id: hub.session_id,
                    ask_id,
                }));
            }
            AgentCommand::Shutdown { reply } => {
                shutdown_reply = Some(reply);
                closing = true;
                break;
            }
        }
    }

    if closing {
        hub.status(AgentStatus::Closing);
    }
    while let Some(command) = backlog.pop_front() {
        fail_pending_command(command, &hub);
    }
    while let Ok(command) = commands.try_recv() {
        fail_pending_command(command, &hub);
    }
    drain_unregistered_asks(
        &mut ask_user_requests,
        "askuser request was cancelled because the agent is closing",
    );
    if let Some(subagents) = subagents {
        subagents.shutdown("parent agent actor stopped").await;
    }
    drop(agent);
    hub.closed();
    if let Some(reply) = shutdown_reply {
        let _ = reply.send(());
    }
}

async fn receive_subagent_event(
    receiver: &mut Option<broadcast::Receiver<SubagentEvent>>,
) -> Result<SubagentEvent, broadcast::error::RecvError> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn handle_subagent_event(
    event: SubagentEvent,
    hub: &EventHub,
    prompt_slots: &Arc<Semaphore>,
    backlog: &mut VecDeque<AgentCommand>,
) {
    let wake_content = subagent_wake_content(&event);
    if matches!(&event.kind, SubagentEventKind::AgentEvent(_)) {
        hub.observe_subagent_stream(&event);
    } else {
        hub.subagent(event);
    }
    let Some(content) = wake_content else {
        return;
    };
    if matches!(
        hub.current_status(),
        AgentStatus::Closing | AgentStatus::Closed
    ) {
        return;
    }
    let queue_permit = match Arc::clone(prompt_slots).try_acquire_owned() {
        Ok(permit) => permit,
        Err(TryAcquireError::NoPermits) => {
            hub.operation_failed(
                "subagent_notification",
                format!(
                    "subagent notification could not wake parent because the prompt queue is full (capacity {COMMAND_CAPACITY})"
                ),
            );
            return;
        }
        Err(TryAcquireError::Closed) => return,
    };
    let run_id = RunId::new();
    hub.run_queued(run_id);
    backlog.push_back(AgentCommand::Prompt {
        run_id,
        content,
        visibility: MessageVisibility::Internal,
        queue_permit,
        admission: None,
    });
}

fn resync_subagents(hub: &EventHub, runtime: Option<&SubagentRuntime>, skipped: u64) {
    let Some(runtime) = runtime else {
        return;
    };
    let subagents = runtime
        .snapshots()
        .iter()
        .map(subagent_summary)
        .collect::<Vec<_>>();
    hub.subagents_resynced(subagents, skipped);
}

fn subagent_wake_content(event: &SubagentEvent) -> Option<Content> {
    if !event.wakes_parent() {
        return None;
    }
    let payload = match &event.kind {
        SubagentEventKind::Notification(notification) => serde_json::json!({
            "type": "subagent_notification",
            "agent_id": event.agent_id,
            "sequence": event.sequence,
            "delivery_id": notification.delivery_id,
            "kind": notification.kind,
            "source": notification.source,
            "message": notification.message,
        }),
        SubagentEventKind::Closed {
            delivery_id,
            reason,
            ..
        } => serde_json::json!({
            "type": "subagent_notification",
            "agent_id": event.agent_id,
            "sequence": event.sequence,
            "delivery_id": delivery_id,
            "kind": SubagentNotificationKind::Closed,
            "source": "runtime",
            "message": reason,
        }),
        _ => return None,
    };
    Some(Content::text(format!(
        "<subagent_notification>{payload}</subagent_notification>"
    )))
}

fn admit_prompt(command: &mut AgentCommand, hub: &EventHub) -> bool {
    let AgentCommand::Prompt {
        run_id, admission, ..
    } = command
    else {
        return true;
    };
    let Some(admission) = admission.take() else {
        return true;
    };
    if matches!(
        hub.current_status(),
        AgentStatus::Closing | AgentStatus::Closed
    ) {
        let _ = admission.send(Err(()));
        return false;
    }
    let position = hub.run_queued(*run_id);
    let _ = admission.send(Ok(position));
    true
}

fn register_pending_ask(
    pending_asks: &mut HashMap<AskUserId, PendingAskUserRequest>,
    hub: &EventHub,
    request: PendingAskUserRequest,
) {
    let ask_id = request.request.ask_id;
    if let Some(previous) = pending_asks.insert(ask_id, request) {
        let _ = previous
            .reply
            .send(Err("askuser request ID was reused".to_owned()));
        hub.ask_user_cancelled(ask_id);
    }
    let request = pending_asks
        .get(&ask_id)
        .expect("the askuser request was just inserted")
        .request
        .clone();
    hub.ask_user_requested(request);
}

fn answer_pending_ask(
    pending_asks: &mut HashMap<AskUserId, PendingAskUserRequest>,
    hub: &EventHub,
    ask_id: AskUserId,
    answers: Vec<AskUserAnswer>,
) -> Result<(), AgentHandleError> {
    let Some(pending) = pending_asks.get(&ask_id) else {
        return Err(AgentHandleError::AskUserNotPending {
            session_id: hub.session_id,
            ask_id,
        });
    };
    validate_answers(&pending.request.questions, &answers)
        .map_err(|message| AgentHandleError::InvalidAskUserAnswer { message })?;

    let pending = pending_asks
        .remove(&ask_id)
        .expect("the validated askuser request must still be pending");
    if pending.reply.send(Ok(answers)).is_err() {
        hub.ask_user_cancelled(ask_id);
        return Err(AgentHandleError::AskUserNotPending {
            session_id: hub.session_id,
            ask_id,
        });
    }
    hub.ask_user_answered(ask_id);
    Ok(())
}

fn cancel_pending_asks(
    pending_asks: &mut HashMap<AskUserId, PendingAskUserRequest>,
    hub: &EventHub,
    message: &str,
) {
    for (ask_id, pending) in pending_asks.drain() {
        let _ = pending.reply.send(Err(message.to_owned()));
        hub.ask_user_cancelled(ask_id);
    }
}

fn cancel_pending_ask(
    pending_asks: &mut HashMap<AskUserId, PendingAskUserRequest>,
    hub: &EventHub,
    ask_id: AskUserId,
    message: &str,
) -> bool {
    let Some(pending) = pending_asks.remove(&ask_id) else {
        return false;
    };
    let _ = pending.reply.send(Err(message.to_owned()));
    hub.ask_user_cancelled(ask_id);
    true
}

fn drain_unregistered_asks(
    ask_user_requests: &mut mpsc::UnboundedReceiver<PendingAskUserRequest>,
    message: &str,
) {
    while let Ok(pending) = ask_user_requests.try_recv() {
        let _ = pending.reply.send(Err(message.to_owned()));
    }
}

fn run_is_stopping(control: &AgentRunControl, hub: &EventHub) -> bool {
    control.is_stopped()
        || matches!(
            hub.current_status(),
            AgentStatus::Stopping | AgentStatus::Closing | AgentStatus::Closed
        )
}

fn fail_pending_command(command: AgentCommand, hub: &EventHub) {
    match command {
        AgentCommand::Prompt {
            run_id,
            queue_permit,
            admission,
            ..
        } => {
            drop(queue_permit);
            if let Some(admission) = admission {
                let _ = admission.send(Err(()));
            } else {
                hub.run_dequeued(run_id);
                hub.run_failed(run_id, "agent is closing".to_owned(), AgentStatus::Closing);
            }
        }
        AgentCommand::Initialize { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::CompactContext { admission, .. } => {
            let _ = admission.send(Err(AgentHandleError::Busy {
                session_id: hub.session_id,
                status: AgentStatus::Closing,
            }));
        }
        AgentCommand::SetModel { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::SetTitle { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::SetPinned { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::SnapshotForFork { reply } => {
            let _ = reply.send(Err(AgentHandleError::Busy {
                session_id: hub.session_id,
                status: AgentStatus::Closing,
            }));
        }
        AgentCommand::SetReasoning { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::SetCapabilityMode { reply, .. } => {
            let _ = reply.send(Err("agent is closing".to_owned()));
        }
        AgentCommand::AnswerAskUser { ask_id, reply, .. } => {
            let _ = reply.send(Err(AgentHandleError::AskUserNotPending {
                session_id: hub.session_id,
                ask_id,
            }));
        }
        AgentCommand::Shutdown { reply } => {
            let _ = reply.send(());
        }
    }
}

async fn apply_model(
    agent: &mut Agent,
    hub: &EventHub,
    binding: &mut Option<MetadataBinding>,
    model: String,
) -> Result<(), String> {
    let revision = hub.config_revision().saturating_add(1);
    if let Some(binding) = binding {
        let mut next = binding.record.clone();
        next.model.clone_from(&model);
        next.config_revision = revision;
        binding
            .store
            .update_session(next.clone())
            .await
            .map_err(|error| error.to_string())?;
        binding.record = next;
    }
    agent.set_model(model.clone());
    hub.config_changed(model, agent.reasoning_effort(), revision);
    Ok(())
}

async fn apply_title(
    hub: &EventHub,
    binding: &mut Option<MetadataBinding>,
    title: String,
) -> Result<(), String> {
    let title = crate::session_title::normalize_title(&title).map_err(|error| error.to_string())?;
    let binding = binding
        .as_mut()
        .ok_or_else(|| "session is not initialized".to_owned())?;
    if binding.record.title.is_some() {
        return Ok(());
    }
    let mut next = binding.record.clone();
    next.title = Some(title.clone());
    binding
        .store
        .update_session(next.clone())
        .await
        .map_err(|error| error.to_string())?;
    binding.record = next;
    hub.title_changed(title);
    Ok(())
}

async fn apply_pinned(binding: &mut Option<MetadataBinding>, pinned: bool) -> Result<(), String> {
    let binding = binding
        .as_mut()
        .ok_or_else(|| "session is not initialized".to_owned())?;
    if binding.record.pinned == pinned {
        return Ok(());
    }
    let mut next = binding.record.clone();
    next.pinned = pinned;
    binding
        .store
        .update_session(next.clone())
        .await
        .map_err(|error| error.to_string())?;
    binding.record = next;
    Ok(())
}

async fn apply_reasoning(
    agent: &mut Agent,
    hub: &EventHub,
    binding: &mut Option<MetadataBinding>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), String> {
    let revision = hub.config_revision().saturating_add(1);
    if let Some(binding) = binding {
        let mut next = binding.record.clone();
        next.reasoning_effort = reasoning_effort;
        next.config_revision = revision;
        binding
            .store
            .update_session(next.clone())
            .await
            .map_err(|error| error.to_string())?;
        binding.record = next;
    }
    agent.set_reasoning_effort(reasoning_effort);
    hub.config_changed(hub.model(), reasoning_effort, revision);
    Ok(())
}

async fn apply_capability_mode(
    agent: &mut Agent,
    hub: &EventHub,
    subagents: Option<&SubagentRuntime>,
    capability_mode: CapabilityMode,
) -> Result<(), String> {
    if let Some(subagents) = subagents {
        subagents
            .close_exceeding_capability(capability_mode, "parent capability mode was narrowed")
            .await;
    }
    let result = agent
        .set_capability_mode(capability_mode)
        .await
        .map_err(|error| error.to_string());
    let effective = agent.capability_mode();
    if let Some(subagents) = subagents {
        subagents.set_capability_ceiling(effective);
    }
    if hub.current_capability_mode() != effective {
        hub.capability_mode_changed(effective);
    }
    result
}

struct EventHub {
    session_id: SessionId,
    publish_lock: Mutex<()>,
    queued_run_ids: Mutex<VecDeque<RunId>>,
    sequence: AtomicU64,
    events: broadcast::Sender<RuntimeEvent>,
    state: watch::Sender<AgentView>,
}

impl EventHub {
    fn current_status(&self) -> AgentStatus {
        self.state.borrow().status
    }

    fn current_capability_mode(&self) -> CapabilityMode {
        self.state.borrow().capability_mode
    }

    fn is_initialized(&self) -> bool {
        self.state.borrow().initialized
    }

    fn config_revision(&self) -> u64 {
        self.state.borrow().config_revision
    }

    fn model(&self) -> String {
        self.state.borrow().model.clone()
    }

    fn publish(&self, kind: RuntimeEventKind, update: impl FnOnce(&mut AgentView)) -> u64 {
        // `publish` is called both by the actor and directly by concurrent WS
        // command handlers. Keep sequence allocation, state projection and
        // broadcast insertion in one critical section so snapshots never
        // regress and every subscriber observes monotonically ordered events.
        let _publish = self.publish_lock.lock().expect("event hub lock poisoned");
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.send_modify(|state| {
            update(state);
            state.last_event_sequence = sequence;
        });
        let run_id = match &kind {
            RuntimeEventKind::RunQueued { run_id }
            | RuntimeEventKind::RunStarted { run_id }
            | RuntimeEventKind::RunCompleted { run_id }
            | RuntimeEventKind::RunStopped { run_id }
            | RuntimeEventKind::RunFailed { run_id, .. } => Some(*run_id),
            _ => self.state.borrow().active_run_id,
        };
        let _ = self.events.send(RuntimeEvent {
            sequence,
            session_id: self.session_id,
            run_id,
            kind,
        });
        sequence
    }

    fn status(&self, status: AgentStatus) {
        self.publish(RuntimeEventKind::StateChanged { status }, |state| {
            state.status = status;
        });
    }

    fn initialized(&self, agent: &Agent, record: &SessionRecord) {
        self.publish(RuntimeEventKind::SessionInitialized, |state| {
            state.initialized = true;
            state.status = AgentStatus::Idle;
            state.title.clone_from(&record.title);
            state.profile_id.clone_from(&record.profile_id);
            state.model.clone_from(&record.model);
            state.reasoning_effort = record.reasoning_effort;
            state.config_revision = record.config_revision;
            state.capability_mode = agent.capability_mode();
            state.messages = agent.messages().to_vec();
            state.display_messages = agent.session_history().messages.clone();
            state.context_compactions = restored_context_compactions(agent);
            state.context_compaction = state
                .context_compactions
                .last()
                .cloned()
                .or_else(|| restored_context_compaction(&state.messages));
            state.last_usage = agent.last_usage();
            state.context_usage = agent.context_usage();
            state.cumulative_usage = agent.cumulative_usage();
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn title_changed(&self, title: String) {
        self.publish(
            RuntimeEventKind::TitleChanged {
                title: title.clone(),
            },
            |state| state.title = Some(title),
        );
    }

    fn run_queued(&self, run_id: RunId) -> usize {
        self.queued_run_ids
            .lock()
            .expect("queued run lock poisoned")
            .push_back(run_id);
        let mut position = 0;
        self.publish(RuntimeEventKind::RunQueued { run_id }, |state| {
            state.queued_runs = state.queued_runs.saturating_add(1);
            position = state.queued_runs;
        });
        position
    }

    fn run_dequeued(&self, run_id: RunId) {
        let mut queued = self
            .queued_run_ids
            .lock()
            .expect("queued run lock poisoned");
        if let Some(index) = queued.iter().position(|queued| *queued == run_id) {
            queued.remove(index);
        }
        drop(queued);
        let status = self.state.borrow().status;
        self.publish(RuntimeEventKind::StateChanged { status }, |state| {
            state.queued_runs = state.queued_runs.saturating_sub(1)
        });
    }

    fn run_started(&self, run_id: RunId) {
        self.publish(RuntimeEventKind::RunStarted { run_id }, |state| {
            state.status = AgentStatus::Running;
            state.active_run_id = Some(run_id);
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn ask_user_requested(&self, request: AskUserRequest) {
        self.publish(
            RuntimeEventKind::AskUserRequested {
                request: request.clone(),
            },
            |state| state.pending_asks.push(request),
        );
    }

    fn ask_user_answered(&self, ask_id: AskUserId) {
        self.publish(RuntimeEventKind::AskUserAnswered { ask_id }, |state| {
            state
                .pending_asks
                .retain(|request| request.ask_id != ask_id);
        });
    }

    fn ask_user_cancelled(&self, ask_id: AskUserId) {
        self.publish(RuntimeEventKind::AskUserCancelled { ask_id }, |state| {
            state
                .pending_asks
                .retain(|request| request.ask_id != ask_id);
        });
    }

    fn run_completed(&self, run_id: RunId, status: AgentStatus) {
        self.publish(RuntimeEventKind::RunCompleted { run_id }, |state| {
            state.status = status;
            state.active_run_id = None;
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn run_stopped(&self, run_id: RunId, status: AgentStatus) {
        self.publish(RuntimeEventKind::RunStopped { run_id }, |state| {
            state.status = status;
            state.active_run_id = None;
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn run_failed(&self, run_id: RunId, message: String, status: AgentStatus) {
        self.publish(RuntimeEventKind::RunFailed { run_id, message }, |state| {
            state.status = status;
            state.active_run_id = None;
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn config_changed(
        &self,
        model: String,
        reasoning_effort: Option<ReasoningEffort>,
        revision: u64,
    ) {
        self.publish(
            RuntimeEventKind::ConfigChanged {
                model: model.clone(),
                reasoning_effort,
                revision,
            },
            |state| {
                state.model = model;
                state.reasoning_effort = reasoning_effort;
                state.config_revision = revision;
            },
        );
    }

    fn capability_mode_changed(&self, capability_mode: CapabilityMode) {
        self.publish(
            RuntimeEventKind::CapabilityModeChanged { capability_mode },
            |state| state.capability_mode = capability_mode,
        );
    }

    fn operation_failed(&self, operation: impl Into<String>, message: String) {
        self.publish(
            RuntimeEventKind::OperationFailed {
                operation: operation.into(),
                message,
            },
            |_| {},
        );
    }

    fn actor_crashed(&self, message: String) {
        self.publish(RuntimeEventKind::ActorCrashed { message }, |state| {
            state.status = AgentStatus::Closing;
            state.draft = None;
            state.pending_asks.clear();
        });
    }

    fn fail_all_queued(&self, message: &str) {
        let queued = self
            .queued_run_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect::<Vec<_>>();
        for run_id in queued {
            self.publish(
                RuntimeEventKind::RunFailed {
                    run_id,
                    message: message.to_owned(),
                },
                |state| {
                    state.status = AgentStatus::Closing;
                    state.queued_runs = state.queued_runs.saturating_sub(1);
                    state.draft = None;
                },
            );
        }
    }

    fn publish_agent(&self, event: &AgentEvent) {
        self.publish(RuntimeEventKind::Agent(wire_agent_event(event)), |state| {
            apply_agent_event(state, event)
        });
    }

    fn subagent(&self, event: SubagentEvent) {
        let projected = event.clone();
        self.publish(RuntimeEventKind::Subagent(event), |state| {
            apply_subagent_event(state, &projected);
        });
    }

    fn observe_subagent_stream(&self, event: &SubagentEvent) {
        let _publish = self.publish_lock.lock().expect("event hub lock poisoned");
        self.state
            .send_modify(|state| apply_subagent_event(state, event));
    }

    fn subagents_resynced(&self, subagents: Vec<SubagentSummary>, skipped: u64) {
        self.publish(
            RuntimeEventKind::SubagentsResynced {
                subagents: subagents.clone(),
            },
            |state| state.subagents = subagents,
        );
        self.operation_failed(
            "subagent_events",
            format!("subagent event receiver lagged by {skipped} events; projection resynced"),
        );
    }

    fn closed(&self) {
        self.publish(
            RuntimeEventKind::StateChanged {
                status: AgentStatus::Closed,
            },
            |state| {
                state.status = AgentStatus::Closed;
                state.active_run_id = None;
                state.queued_runs = 0;
                state.draft = None;
                state.pending_asks.clear();
            },
        );
    }
}

fn subagent_summary(snapshot: &SubagentSnapshot) -> SubagentSummary {
    SubagentSummary {
        agent_id: snapshot.agent_id.clone(),
        description: snapshot.description.clone(),
        state: snapshot.state.clone(),
        last_sequence: snapshot.last_sequence,
    }
}

fn apply_subagent_event(state: &mut AgentView, event: &SubagentEvent) {
    let existing = state
        .subagents
        .iter_mut()
        .find(|summary| summary.agent_id == event.agent_id);
    let summary = match existing {
        Some(summary) => summary,
        None => {
            let description = match &event.kind {
                SubagentEventKind::Spawned { description, .. } => description.clone(),
                _ => String::new(),
            };
            state.subagents.push(SubagentSummary {
                agent_id: event.agent_id.clone(),
                description,
                state: SubagentState::Starting,
                last_sequence: 0,
            });
            state
                .subagents
                .last_mut()
                .expect("a subagent summary was just inserted")
        }
    };
    if let SubagentEventKind::Spawned { description, .. } = &event.kind {
        summary.description.clone_from(description);
    }
    match &event.kind {
        SubagentEventKind::StateChanged { state } => summary.state = state.clone(),
        SubagentEventKind::Closed { .. } => summary.state = SubagentState::Closed,
        _ => {}
    }
    summary.last_sequence = event.sequence;
    state
        .subagents
        .sort_unstable_by(|left, right| left.agent_id.cmp(&right.agent_id));
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return format!("agent actor panicked: {message}");
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return format!("agent actor panicked: {message}");
    }
    "agent actor panicked".to_owned()
}

fn wire_agent_event(event: &AgentEvent) -> AgentEvent {
    match event {
        // Full transcripts are projected into the watch snapshot above. They
        // are deliberately omitted from the broadcast ring because the WS
        // protocol represents these as terminal markers, not history payloads.
        AgentEvent::AgentEnd { .. } => AgentEvent::AgentEnd {
            messages: Vec::new(),
        },
        AgentEvent::AgentStopped { .. } => AgentEvent::AgentStopped {
            messages: Vec::new(),
        },
        AgentEvent::ContextCompactionStarted {
            trigger, compactor, ..
        } => AgentEvent::ContextCompactionStarted {
            trigger: trigger.clone(),
            compactor: compactor.clone(),
            prompt: String::new(),
        },
        AgentEvent::ContextCompactionCompleted {
            trigger,
            compactor,
            before_message_count,
            after_message_count,
            changed_from,
            usage,
            estimated_context_tokens,
            ..
        } => AgentEvent::ContextCompactionCompleted {
            trigger: trigger.clone(),
            compactor: compactor.clone(),
            before_message_count: *before_message_count,
            after_message_count: *after_message_count,
            changed_from: *changed_from,
            replacement: Vec::new(),
            summary: String::new(),
            usage: *usage,
            estimated_context_tokens: *estimated_context_tokens,
        },
        event => event.clone(),
    }
}

fn restored_context_compaction(messages: &[Message]) -> Option<ContextCompactionView> {
    let boundary_index = messages
        .windows(2)
        .enumerate()
        .rev()
        .find_map(|(index, pair)| {
            let boundary = &pair[0];
            let summary = &pair[1];
            (boundary.role == Role::System
                && boundary.visibility == MessageVisibility::Internal
                && boundary.text_content() == Some(DEFAULT_CONTEXT_COMPACTION_BOUNDARY_MESSAGE)
                && summary.role == Role::User
                && summary.visibility == MessageVisibility::Internal)
                .then_some(index)
        })?;
    let after_message_count = boundary_index + 2;
    Some(ContextCompactionView {
        phase: ContextCompactionPhase::Completed,
        history_index: after_message_count,
        hidden_range: Some(boundary_index..after_message_count),
        after_message_count: Some(after_message_count),
        message: None,
    })
}

fn restored_context_compactions(agent: &Agent) -> Vec<ContextCompactionView> {
    agent
        .session_history()
        .compactions
        .iter()
        .map(|compaction| ContextCompactionView {
            phase: ContextCompactionPhase::Completed,
            history_index: compaction.history_index,
            hidden_range: Some(compaction.changed_from..compaction.after_message_count),
            after_message_count: Some(compaction.after_message_count),
            message: None,
        })
        .collect()
}

fn apply_agent_event(state: &mut AgentView, event: &AgentEvent) {
    match event {
        AgentEvent::AgentStart => state.draft = None,
        AgentEvent::AgentEnd { messages } | AgentEvent::AgentStopped { messages } => {
            state.messages.clone_from(messages);
            state.draft = None;
        }
        AgentEvent::MessageStart { message } if message.role == Role::Assistant => {
            state.draft = Some(AssistantDraft::default());
        }
        AgentEvent::MessageUpdate { delta } => apply_delta(state, delta),
        AgentEvent::MessageEnd { message } => {
            state.messages.push(message.clone());
            state.display_messages.push(message.clone());
            if message.role == Role::Assistant {
                state.draft = None;
            }
        }
        AgentEvent::MessageAborted => state.draft = None,
        AgentEvent::ToolExecutionStart { call } => {
            // The library emits ToolExecutionStart only after its protocol
            // journal is durable. The next active transcript index is now a
            // safe before-tools fork boundary for snapshots and reconnects.
            let fork_message_index = state.messages.len();
            let draft = state.draft.get_or_insert_with(AssistantDraft::default);
            draft.fork_message_index = Some(fork_message_index);
            if !draft
                .tool_calls
                .iter()
                .any(|tool_call| tool_call.id.as_deref() == Some(call.id.as_str()))
            {
                draft.tool_calls.push(ToolCallDraft {
                    index: draft.tool_calls.len(),
                    id: Some(call.id.clone()),
                    name: Some(call.name.clone()),
                    arguments: call.arguments.to_string(),
                });
            }
        }
        AgentEvent::TurnEnd { message, .. } => {
            if let Some(current) = state
                .messages
                .iter_mut()
                .rev()
                .find(|current| current.role == Role::Assistant)
            {
                current.clone_from(message);
            }
            if let Some(current) = state
                .display_messages
                .iter_mut()
                .rev()
                .find(|current| current.role == Role::Assistant)
            {
                current.clone_from(message);
            }
        }
        AgentEvent::UsageUpdate {
            usage,
            context_usage,
        } => {
            state.last_usage = Some(*usage);
            state.context_usage = *context_usage;
            state.cumulative_usage += *usage;
        }
        AgentEvent::ContextCompactionStarted { .. } => {
            state.context_compaction = Some(ContextCompactionView {
                phase: ContextCompactionPhase::Started,
                history_index: state.display_messages.len(),
                hidden_range: None,
                after_message_count: None,
                message: None,
            });
        }
        AgentEvent::ContextCompactionCompleted {
            after_message_count,
            changed_from,
            replacement,
            usage,
            ..
        } => {
            if *changed_from <= state.messages.len() {
                state.messages.truncate(*changed_from);
                state.messages.extend_from_slice(replacement);
                debug_assert_eq!(state.messages.len(), *after_message_count);
            }
            state.last_usage = None;
            state.context_usage = None;
            if let Some(usage) = usage {
                state.cumulative_usage += *usage;
            }
            let completed = ContextCompactionView {
                phase: ContextCompactionPhase::Completed,
                history_index: state.display_messages.len(),
                hidden_range: Some(*changed_from..*after_message_count),
                after_message_count: Some(*after_message_count),
                message: None,
            };
            state.context_compactions.push(completed.clone());
            state.context_compaction = Some(completed);
        }
        AgentEvent::ContextCompactionFailed { message, .. } => {
            let history_index = state
                .context_compaction
                .as_ref()
                .map_or(state.messages.len(), |compaction| compaction.history_index);
            state.context_compaction = Some(ContextCompactionView {
                phase: ContextCompactionPhase::Failed,
                history_index,
                hidden_range: None,
                after_message_count: None,
                message: Some(message.clone()),
            });
        }
        AgentEvent::TurnStart { .. }
        | AgentEvent::MessageStart { .. }
        | AgentEvent::ToolExecutionProgress { .. }
        | AgentEvent::ToolExecutionEnd { .. }
        | AgentEvent::ProviderRetry { .. }
        | AgentEvent::Error { .. } => {}
    }
}

fn apply_delta(state: &mut AgentView, delta: &AssistantDelta) {
    let draft = state.draft.get_or_insert_with(AssistantDraft::default);
    match delta {
        AssistantDelta::Reasoning { delta } => draft.reasoning.push_str(delta),
        AssistantDelta::Text { delta } => draft.text.push_str(delta),
        AssistantDelta::ToolCall {
            index,
            id,
            name,
            arguments_delta,
        } => {
            let tool_call = match draft
                .tool_calls
                .iter_mut()
                .find(|tool_call| tool_call.index == *index)
            {
                Some(tool_call) => tool_call,
                None => {
                    draft.tool_calls.push(ToolCallDraft {
                        index: *index,
                        id: None,
                        name: None,
                        arguments: String::new(),
                    });
                    draft
                        .tool_calls
                        .last_mut()
                        .expect("a tool-call draft was just inserted")
                }
            };
            if id.is_some() {
                tool_call.id.clone_from(id);
            }
            if name.is_some() {
                tool_call.name.clone_from(name);
            }
            tool_call.arguments.push_str(arguments_delta);
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentHandleError {
    #[error("agent actor for session {session_id} is not running")]
    ActorStopped { session_id: SessionId },

    #[error("agent actor for session {session_id} dropped its response")]
    ResponseDropped { session_id: SessionId },

    #[error("agent command queue for session {session_id} is full (capacity {capacity})")]
    QueueFull {
        session_id: SessionId,
        capacity: usize,
    },

    #[error("session {session_id} is busy in state {status:?}")]
    Busy {
        session_id: SessionId,
        status: AgentStatus,
    },

    #[error("session {session_id} has no active run")]
    NoActiveRun { session_id: SessionId },

    #[error(
        "session {session_id} is running {active}, so stop request for {requested} was rejected"
    )]
    RunMismatch {
        session_id: SessionId,
        active: RunId,
        requested: RunId,
    },

    #[error("invalid agent command: {message}")]
    InvalidCommand { message: String },

    #[error("session {session_id} is not waiting for askuser request {ask_id}")]
    AskUserNotPending {
        session_id: SessionId,
        ask_id: AskUserId,
    },

    #[error("invalid askuser answer: {message}")]
    InvalidAskUserAnswer { message: String },

    #[error("agent operation for session {session_id} failed: {message}")]
    Operation {
        session_id: SessionId,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use phi::{SubagentNotification, SubagentNotificationSource};

    fn test_hub() -> EventHub {
        let session_id = SessionId::new();
        let (events, _) = broadcast::channel(16);
        let (state, _) = watch::channel(AgentView {
            session_id,
            title: None,
            profile_id: "test".to_owned(),
            agent_profile_id: crate::runtime::DEFAULT_AGENT_PROFILE_ID.to_owned(),
            agent_profile_revision: crate::runtime::DEFAULT_AGENT_PROFILE_REVISION,
            initialized: true,
            status: AgentStatus::Idle,
            active_run_id: None,
            queued_runs: 0,
            capability_mode: CapabilityMode::FullAccess,
            model: "test-model".to_owned(),
            workspace: None,
            reasoning_effort: None,
            config_revision: 0,
            display_messages: Vec::new(),
            messages: Vec::new(),
            context_compactions: Vec::new(),
            context_compaction: None,
            draft: None,
            last_usage: None,
            context_usage: None,
            cumulative_usage: TokenUsage::default(),
            pending_asks: Vec::new(),
            subagents: Vec::new(),
            last_event_sequence: 0,
        });
        EventHub {
            session_id,
            publish_lock: Mutex::new(()),
            queued_run_ids: Mutex::new(VecDeque::new()),
            sequence: AtomicU64::new(0),
            events,
            state,
        }
    }

    #[test]
    fn restores_the_default_internal_compaction_boundary() {
        let messages = vec![
            Message::system(DEFAULT_CONTEXT_COMPACTION_BOUNDARY_MESSAGE)
                .with_visibility(MessageVisibility::Internal),
            Message::user("private summary").with_visibility(MessageVisibility::Internal),
            Message::user("continue"),
        ];

        assert_eq!(
            restored_context_compaction(&messages),
            Some(ContextCompactionView {
                phase: ContextCompactionPhase::Completed,
                history_index: 2,
                hidden_range: Some(0..2),
                after_message_count: Some(2),
                message: None,
            })
        );
    }

    #[test]
    fn compaction_replaces_active_context_but_retains_display_history() {
        let hub = test_hub();
        let mut state = hub.state.borrow().clone();
        let visible = vec![
            Message::user("old question"),
            Message::assistant(Some(Content::text("old answer")), Vec::new()),
        ];
        state.messages.clone_from(&visible);
        state.display_messages.clone_from(&visible);
        let replacement = vec![
            Message::system(DEFAULT_CONTEXT_COMPACTION_BOUNDARY_MESSAGE)
                .with_visibility(MessageVisibility::Internal),
            Message::user("private summary").with_visibility(MessageVisibility::Internal),
        ];

        apply_agent_event(
            &mut state,
            &AgentEvent::ContextCompactionCompleted {
                trigger: phi::ContextCompactionTrigger::Manual { instructions: None },
                compactor: "default".to_owned(),
                before_message_count: visible.len(),
                after_message_count: replacement.len(),
                changed_from: 0,
                replacement: replacement.clone(),
                summary: "must stay private".to_owned(),
                usage: None,
                estimated_context_tokens: 10,
            },
        );

        assert_eq!(state.messages, replacement);
        assert_eq!(state.display_messages, visible);
        assert_eq!(
            state.context_compactions,
            vec![ContextCompactionView {
                phase: ContextCompactionPhase::Completed,
                history_index: 2,
                hidden_range: Some(0..2),
                after_message_count: Some(2),
                message: None,
            }]
        );
    }

    fn notification_event(
        sequence: u64,
        kind: SubagentNotificationKind,
        wake_parent: bool,
    ) -> SubagentEvent {
        SubagentEvent {
            sequence,
            parent_id: "parent".to_owned(),
            agent_id: "agent-1".to_owned(),
            kind: SubagentEventKind::Notification(SubagentNotification {
                delivery_id: format!("delivery-{sequence}"),
                kind,
                source: SubagentNotificationSource::Child,
                message: "status".to_owned(),
                wake_parent,
            }),
        }
    }

    #[test]
    fn reasoning_delta_updates_the_live_assistant_draft() {
        let hub = test_hub();
        let mut state = hub.state.borrow().clone();

        apply_agent_event(
            &mut state,
            &AgentEvent::MessageStart {
                message: Message::assistant(None, Vec::new()),
            },
        );
        apply_agent_event(
            &mut state,
            &AgentEvent::MessageUpdate {
                delta: AssistantDelta::Reasoning {
                    delta: "inspect inputs".to_owned(),
                },
            },
        );

        assert_eq!(
            state.draft.as_ref().map(|draft| draft.reasoning.as_str()),
            Some("inspect inputs")
        );
    }

    #[tokio::test]
    async fn pinned_updates_are_persisted_through_the_actor_metadata_binding() {
        let store = Arc::new(crate::store::MemoryControlStore::new());
        let record = SessionRecord::new(SessionId::new(), "default", "test-model", None);
        store.create_session(record.clone()).await.unwrap();
        let mut binding = Some(MetadataBinding {
            record: record.clone(),
            store: store.clone(),
        });

        apply_pinned(&mut binding, true).await.unwrap();

        assert!(binding.as_ref().unwrap().record.pinned);
        assert!(store.get_session(record.id).await.unwrap().unwrap().pinned);
    }

    #[test]
    fn spawned_subagent_is_published_through_parent_hub() {
        let hub = test_hub();
        let mut events = hub.events.subscribe();
        hub.subagent(SubagentEvent {
            sequence: 1,
            parent_id: "parent".to_owned(),
            agent_id: "agent-1".to_owned(),
            kind: SubagentEventKind::Spawned {
                description: "research".to_owned(),
                initial_delivery_id: "delivery-1".to_owned(),
                effective_config: phi::EffectiveSubagentConfig {
                    agent_type: phi::SubagentType::Explore,
                    capability_mode: CapabilityMode::ReadOnly,
                    generation_config: phi::GenerationConfig::default(),
                    output_contract: phi::SubagentOutputContract::Text,
                    isolation: phi::SubagentIsolation::Shared,
                },
            },
        });

        let published = events.try_recv().expect("spawn event should be broadcast");
        assert!(matches!(
            published.kind,
            RuntimeEventKind::Subagent(SubagentEvent {
                kind: SubagentEventKind::Spawned { .. },
                ..
            })
        ));
        assert_eq!(
            hub.state.borrow().subagents,
            vec![SubagentSummary {
                agent_id: "agent-1".to_owned(),
                description: "research".to_owned(),
                state: SubagentState::Starting,
                last_sequence: 1,
            }]
        );
    }

    #[test]
    fn progress_is_observable_without_waking_and_result_is_queued() {
        let hub = test_hub();
        let mut events = hub.events.subscribe();
        let prompt_slots = Arc::new(Semaphore::new(2));
        let mut backlog = VecDeque::new();

        handle_subagent_event(
            notification_event(1, SubagentNotificationKind::Progress, false),
            &hub,
            &prompt_slots,
            &mut backlog,
        );
        assert!(backlog.is_empty());
        assert_eq!(hub.state.borrow().queued_runs, 0);
        assert!(matches!(
            events
                .try_recv()
                .expect("progress should be observable")
                .kind,
            RuntimeEventKind::Subagent(SubagentEvent {
                kind: SubagentEventKind::Notification(SubagentNotification {
                    kind: SubagentNotificationKind::Progress,
                    ..
                }),
                ..
            })
        ));

        handle_subagent_event(
            notification_event(2, SubagentNotificationKind::Result, true),
            &hub,
            &prompt_slots,
            &mut backlog,
        );
        assert_eq!(backlog.len(), 1);
        assert_eq!(hub.state.borrow().queued_runs, 1);
        let AgentCommand::Prompt {
            content,
            visibility,
            ..
        } = backlog.pop_front().unwrap()
        else {
            panic!("result notification must queue an internal prompt");
        };
        assert_eq!(visibility, MessageVisibility::Internal);
        let content = content.as_text().expect("wake prompt should be text");
        assert!(content.contains("subagent_notification"));
        assert!(content.contains("agent-1"));
        assert!(content.contains("result"));
    }

    #[test]
    fn raw_child_stream_is_not_rebroadcast_to_parent() {
        let hub = test_hub();
        let mut events = hub.events.subscribe();
        let prompt_slots = Arc::new(Semaphore::new(1));
        let mut backlog = VecDeque::new();
        handle_subagent_event(
            SubagentEvent {
                sequence: 7,
                parent_id: "parent".to_owned(),
                agent_id: "agent-1".to_owned(),
                kind: SubagentEventKind::AgentEvent(AgentEvent::AgentStart),
            },
            &hub,
            &prompt_slots,
            &mut backlog,
        );

        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(backlog.is_empty());
        assert_eq!(hub.state.borrow().subagents[0].last_sequence, 7);
    }
}
