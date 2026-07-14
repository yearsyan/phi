use phi::Agent;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::SessionId;

const COMMAND_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    pub session_id: SessionId,
    pub status: AgentStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    StateChanged { state: AgentView },
}

/// Cheap, cloneable handle for one actor-owned `phi::Agent`.
#[derive(Clone)]
pub struct AgentHandle {
    session_id: SessionId,
    commands: mpsc::Sender<AgentCommand>,
    events: broadcast::Sender<RuntimeEvent>,
    state: watch::Receiver<AgentView>,
}

impl AgentHandle {
    pub fn spawn(session_id: SessionId, agent: Agent) -> Self {
        let initial = AgentView {
            session_id,
            status: AgentStatus::Idle,
        };
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (state_sender, state) = watch::channel(initial);

        tokio::spawn(run_actor(
            agent,
            command_receiver,
            events.clone(),
            state_sender,
        ));

        Self {
            session_id,
            commands,
            events,
            state,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn snapshot(&self) -> AgentView {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    pub async fn shutdown(&self) -> Result<(), AgentHandleError> {
        if self.snapshot().status == AgentStatus::Stopped {
            return Ok(());
        }

        let (reply, response) = oneshot::channel();
        self.commands
            .send(AgentCommand::Shutdown { reply })
            .await
            .map_err(|_| AgentHandleError::ActorStopped {
                session_id: self.session_id,
            })?;
        response
            .await
            .map_err(|_| AgentHandleError::ResponseDropped {
                session_id: self.session_id,
            })
    }
}

enum AgentCommand {
    Shutdown { reply: oneshot::Sender<()> },
}

async fn run_actor(
    agent: Agent,
    mut commands: mpsc::Receiver<AgentCommand>,
    events: broadcast::Sender<RuntimeEvent>,
    state: watch::Sender<AgentView>,
) {
    let shutdown_reply = match commands.recv().await {
        Some(AgentCommand::Shutdown { reply }) => {
            publish_state(&events, &state, AgentStatus::Stopping);
            Some(reply)
        }
        None => None,
    };

    drop(agent);
    publish_state(&events, &state, AgentStatus::Stopped);
    if let Some(reply) = shutdown_reply {
        let _ = reply.send(());
    }
}

fn publish_state(
    events: &broadcast::Sender<RuntimeEvent>,
    state: &watch::Sender<AgentView>,
    status: AgentStatus,
) {
    let next = AgentView {
        session_id: state.borrow().session_id,
        status,
    };
    state.send_replace(next.clone());
    let _ = events.send(RuntimeEvent::StateChanged { state: next });
}

#[derive(Debug, Error)]
pub enum AgentHandleError {
    #[error("agent actor for session {session_id} is not running")]
    ActorStopped { session_id: SessionId },

    #[error("agent actor for session {session_id} dropped its response")]
    ResponseDropped { session_id: SessionId },
}
