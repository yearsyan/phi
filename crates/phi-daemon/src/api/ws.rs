use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use phi::{CapabilityMode, Content, SkillInvocation, Workspace};
use serde::Deserialize;
use tokio::sync::broadcast;

use super::{
    AppState,
    auth::{self, WS_AUTH_PROTOCOL_PREFIX, WS_PROTOCOL},
    dto::{
        AgentProfileRefDto, ClientCommand, ServerMessage, SessionConfigDto, SessionDto,
        SkillSummaryDto, SubagentEventDto, SubagentSnapshotDto,
    },
    workspace::resolve_workspace_path,
};
use crate::{
    runtime::DEFAULT_AGENT_PROFILE_ID,
    runtime::{
        AgentHandle, AgentHandleError, AgentStatus, RuntimeEvent, RuntimeEventKind, SessionId,
    },
    service::{ApplicationService, PreparedSession},
    store::DEFAULT_PROFILE_ID,
};

const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024;
const WS_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/ws/new", get(new_session))
        .route("/v1/ws/attach/{session_id}", get(attach_session))
        .route(
            "/v1/ws/attach/{parent_session_id}/subagents/{agent_id}",
            get(attach_subagent),
        )
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSessionQuery {
    profile_id: Option<String>,
    agent_profile_id: Option<String>,
    capability_mode: Option<CapabilityMode>,
    workspace: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachSessionQuery {}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachSubagentQuery {}

async fn new_session(
    State(state): State<AppState>,
    Query(query): Query<NewSessionQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !authenticate_websocket(&headers, &state) {
        return auth::unauthorized_response();
    }
    let workspace = match query.workspace {
        Some(path) => match resolve_workspace_path(path.as_ref()).await {
            Ok(workspace) => Some(workspace),
            Err(error) => return error.into_response(),
        },
        None => None,
    };
    let service = Arc::clone(state.service());
    let profile_id = query
        .profile_id
        .unwrap_or_else(|| DEFAULT_PROFILE_ID.to_owned());
    let agent_profile_id = query
        .agent_profile_id
        .unwrap_or_else(|| DEFAULT_AGENT_PROFILE_ID.to_owned());
    let capability_mode = query.capability_mode;
    websocket
        .protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            handle_new(
                socket,
                service,
                profile_id,
                agent_profile_id,
                capability_mode,
                workspace,
            )
        })
}

async fn attach_session(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    Query(_query): Query<AttachSessionQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !authenticate_websocket(&headers, &state) {
        return auth::unauthorized_response();
    }
    let service = Arc::clone(state.service());
    websocket
        .protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_attach(socket, service, session_id))
}

async fn attach_subagent(
    State(state): State<AppState>,
    Path((parent_session_id, agent_id)): Path<(SessionId, String)>,
    Query(_query): Query<AttachSubagentQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !authenticate_websocket(&headers, &state) {
        return auth::unauthorized_response();
    }
    let service = Arc::clone(state.service());
    websocket
        .protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            handle_subagent_attach(socket, service, parent_session_id, agent_id)
        })
}

/// Browser WebSocket APIs cannot set an Authorization header. They can,
/// however, offer multiple subprotocols. The client offers the fixed public
/// protocol plus a credential protocol, while the server selects and echoes
/// only the fixed value so the bearer token never appears in the response.
fn authenticate_websocket(headers: &HeaderMap, state: &AppState) -> bool {
    let mut supports_protocol = false;
    let mut token = None;

    for value in headers.get_all(header::SEC_WEBSOCKET_PROTOCOL) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for offered in value.split(',').map(str::trim) {
            if offered == WS_PROTOCOL {
                supports_protocol = true;
                continue;
            }
            if let Some(candidate) = offered.strip_prefix(WS_AUTH_PROTOCOL_PREFIX)
                && (candidate.is_empty() || token.replace(candidate).is_some())
            {
                return false;
            }
        }
    }

    supports_protocol && token.is_some_and(|token| state.auth().consume_ws_token(token))
}

async fn handle_new(
    socket: WebSocket,
    service: Arc<ApplicationService>,
    profile_id: String,
    agent_profile_id: String,
    capability_mode: Option<CapabilityMode>,
    workspace: Option<Workspace>,
) {
    let (mut sender, receiver) = socket.split();
    if send_json(&mut sender, &ServerMessage::Building)
        .await
        .is_err()
    {
        return;
    }

    let prepared = match workspace {
        Some(workspace) => {
            service
                .prepare_session_configured_in_workspace(
                    profile_id,
                    agent_profile_id,
                    capability_mode,
                    workspace,
                )
                .await
        }
        None => {
            service
                .prepare_session_configured(profile_id, agent_profile_id, capability_mode)
                .await
        }
    };
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = send_json(
                &mut sender,
                &ServerMessage::FatalError {
                    code: "agent_build_failed",
                    message: error.to_string(),
                },
            )
            .await;
            return;
        }
    };
    let handle = prepared.handle().clone();
    let events = handle.subscribe();
    let summary = handle.summary();
    if send_json(
        &mut sender,
        &ServerMessage::Ready {
            config: SessionConfigDto::from_summary(&summary),
            capability_mode: summary.capability_mode,
            agent_profile: AgentProfileRefDto {
                agent_profile_id: summary.agent_profile_id,
                revision: summary.agent_profile_revision,
            },
            workspace: summary.workspace.as_ref().map(ToString::to_string),
            skills: handle.skills().iter().map(SkillSummaryDto::from).collect(),
        },
    )
    .await
    .is_err()
    {
        service.discard_prepared(&prepared).await;
        return;
    }

    let cleanup_service = Arc::clone(&service);
    let cleanup_prepared = prepared.clone();
    let activated = socket_loop(
        sender,
        receiver,
        events,
        handle.clone(),
        Some(PendingActivation {
            service,
            prepared,
            activated: false,
        }),
        ConnectionKind::New,
        summary.last_event_sequence,
    )
    .await;
    if !activated {
        cleanup_service.discard_prepared(&cleanup_prepared).await;
    }
}

async fn handle_attach(socket: WebSocket, service: Arc<ApplicationService>, session_id: SessionId) {
    let (mut sender, receiver) = socket.split();
    let handle = match service.attach_session(session_id).await {
        Ok(handle) => handle,
        Err(error) => {
            let _ = send_json(
                &mut sender,
                &ServerMessage::FatalError {
                    code: "attach_failed",
                    message: error.to_string(),
                },
            )
            .await;
            return;
        }
    };

    // Subscribe before taking the snapshot. Events that race with the snapshot
    // are buffered and deduplicated by sequence in `socket_loop`.
    let events = handle.subscribe();
    let snapshot = handle.snapshot();
    if send_json(
        &mut sender,
        &ServerMessage::Snapshot {
            session: SessionDto::from_view_with_skills(&snapshot, handle.skills()),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    socket_loop(
        sender,
        receiver,
        events,
        handle,
        None,
        ConnectionKind::Attach,
        snapshot.last_event_sequence,
    )
    .await;
}

async fn handle_subagent_attach(
    socket: WebSocket,
    service: Arc<ApplicationService>,
    parent_session_id: SessionId,
    agent_id: String,
) {
    let (mut sender, receiver) = socket.split();
    let handle = match service.attach_session(parent_session_id).await {
        Ok(handle) => handle,
        Err(error) => {
            let _ = send_json(
                &mut sender,
                &ServerMessage::FatalError {
                    code: "attach_failed",
                    message: error.to_string(),
                },
            )
            .await;
            return;
        }
    };
    let Some(runtime) = handle.subagents().cloned() else {
        let _ = send_json(
            &mut sender,
            &ServerMessage::FatalError {
                code: "subagents_disabled",
                message: "subagents are disabled for this session".to_owned(),
            },
        )
        .await;
        return;
    };

    // Subscribe before taking the child snapshot. Events racing with the
    // snapshot are buffered and deduplicated by the child sequence.
    let events = runtime.subscribe();
    let Some(snapshot) = runtime.snapshot(&agent_id) else {
        let _ = send_json(
            &mut sender,
            &ServerMessage::FatalError {
                code: "subagent_not_found",
                message: format!("subagent `{agent_id}` was not found"),
            },
        )
        .await;
        return;
    };
    if snapshot.parent_id != parent_session_id.to_string() {
        let _ = send_json(
            &mut sender,
            &ServerMessage::FatalError {
                code: "subagent_not_found",
                message: format!("subagent `{agent_id}` was not found"),
            },
        )
        .await;
        return;
    }
    let last_sequence = snapshot.last_sequence;
    let closed = snapshot.state == phi::SubagentState::Closed;
    if send_json(
        &mut sender,
        &ServerMessage::SubagentSnapshot {
            subagent: SubagentSnapshotDto::from(&snapshot),
            input_allowed: false,
        },
    )
    .await
    .is_err()
        || closed
    {
        return;
    }

    subagent_observer_loop(
        sender,
        receiver,
        events,
        runtime,
        parent_session_id,
        agent_id,
        last_sequence,
    )
    .await;
}

async fn subagent_observer_loop(
    mut sender: SplitSink<WebSocket, Message>,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    mut events: broadcast::Receiver<phi::SubagentEvent>,
    runtime: phi::SubagentRuntime,
    parent_session_id: SessionId,
    agent_id: String,
    mut skip_through: u64,
) {
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(incoming) = incoming else { break };
                match incoming {
                    Ok(Message::Ping(payload)) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(Message::Text(_) | Message::Binary(_)) => {
                        let close = Message::Close(Some(CloseFrame {
                            code: close_code::POLICY,
                            reason: "read_only_subagent_stream".into(),
                        }));
                        let _ = tokio::time::timeout(WS_WRITE_TIMEOUT, sender.send(close)).await;
                        break;
                    }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if event.sequence <= skip_through {
                            continue;
                        }
                        skip_through = event.sequence;
                        if event.agent_id != agent_id
                            || event.parent_id != parent_session_id.to_string()
                        {
                            continue;
                        }
                        let closed = matches!(&event.kind, phi::SubagentEventKind::Closed { .. });
                        let message = ServerMessage::SubagentEvent {
                            sequence: event.sequence,
                            parent_session_id: event.parent_id,
                            agent_id: event.agent_id,
                            event: SubagentEventDto::from(event.kind),
                        };
                        if send_json(&mut sender, &message).await.is_err() {
                            break;
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let Some(snapshot) = runtime.snapshot(&agent_id) else {
                            let _ = send_json(
                                &mut sender,
                                &ServerMessage::FatalError {
                                    code: "subagent_not_found",
                                    message: format!("subagent `{agent_id}` was not found"),
                                },
                            )
                            .await;
                            break;
                        };
                        if snapshot.parent_id != parent_session_id.to_string() {
                            break;
                        }
                        let closed = snapshot.state == phi::SubagentState::Closed;
                        skip_through = snapshot.last_sequence;
                        if send_json(
                            &mut sender,
                            &ServerMessage::SubagentResyncRequired {
                                skipped,
                                subagent: SubagentSnapshotDto::from(&snapshot),
                                input_allowed: false,
                            },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

struct PendingActivation {
    service: Arc<ApplicationService>,
    prepared: PreparedSession,
    activated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionKind {
    New,
    Attach,
}

async fn socket_loop(
    mut sender: SplitSink<WebSocket, Message>,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    mut events: broadcast::Receiver<RuntimeEvent>,
    handle: AgentHandle,
    mut pending: Option<PendingActivation>,
    mut connection_kind: ConnectionKind,
    mut skip_through: u64,
) -> bool {
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(incoming) = incoming else { break };
                match incoming {
                    Ok(Message::Text(text)) => {
                        let command = match serde_json::from_str::<ClientCommand>(&text) {
                            Ok(command) => command,
                            Err(error) => {
                                if send_json(&mut sender, &ServerMessage::CommandRejected {
                                    request_id: String::new(),
                                    code: "invalid_command",
                                    message: error.to_string(),
                                }).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        if handle_command(
                            &mut sender,
                            &handle,
                            &mut pending,
                            &mut connection_kind,
                            command,
                        )
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(Message::Binary(_) | Message::Pong(_)) => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) if event.sequence <= skip_through => {
                        if handle.status() == AgentStatus::Closed {
                            break;
                        }
                    }
                    Ok(event) => {
                        skip_through = event.sequence;
                        let closed = matches!(
                            &event.kind,
                            RuntimeEventKind::StateChanged {
                                status: AgentStatus::Closed
                            }
                        );
                        if send_json(&mut sender, &ServerMessage::from(event)).await.is_err() {
                            break;
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let snapshot = handle.snapshot();
                        let closed = snapshot.status == AgentStatus::Closed;
                        skip_through = snapshot.last_event_sequence;
                        if send_json(&mut sender, &ServerMessage::ResyncRequired {
                            skipped,
                            session: SessionDto::from_view_with_skills(
                                &snapshot,
                                handle.skills(),
                            ),
                        }).await.is_err() {
                            break;
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    pending.as_ref().is_none_or(|pending| pending.activated)
}

async fn handle_command(
    sender: &mut SplitSink<WebSocket, Message>,
    handle: &AgentHandle,
    pending: &mut Option<PendingActivation>,
    connection_kind: &mut ConnectionKind,
    command: ClientCommand,
) -> Result<(), ()> {
    let request_id = command.request_id().to_owned();
    match command {
        ClientCommand::Prompt { content, skill, .. } => {
            let title_content = prompt_title_content(&content, skill.as_ref());
            let content = match handle.prepare_prompt(content, skill.as_ref()) {
                Ok(content) => content,
                Err(error) => return reject_handle(sender, request_id, error).await,
            };
            let queued = if let Some(pending) = pending.as_mut()
                && !pending.activated
            {
                match pending
                    .service
                    .activate_and_enqueue_with_title_content(
                        &pending.prepared,
                        content,
                        title_content,
                    )
                    .await
                {
                    Ok((_, queued)) => {
                        pending.activated = true;
                        *connection_kind = ConnectionKind::Attach;
                        send_json(
                            sender,
                            &ServerMessage::SessionCreated {
                                session_id: handle.session_id(),
                            },
                        )
                        .await?;
                        Ok(queued)
                    }
                    Err(error) => {
                        return reject(
                            sender,
                            request_id,
                            "session_activation_failed",
                            error.to_string(),
                        )
                        .await;
                    }
                }
            } else {
                handle.enqueue_prompt(content).await
            };

            match queued {
                Ok(queued) => {
                    send_json(
                        sender,
                        &ServerMessage::CommandAccepted {
                            request_id,
                            command: "prompt",
                            run_id: Some(queued.run_id),
                            queue_position: Some(queued.position),
                        },
                    )
                    .await
                }
                Err(error) => reject_handle(sender, request_id, error).await,
            }
        }
        ClientCommand::Stop { run_id, .. } => match handle.stop(run_id) {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "stop",
                        run_id: Some(run_id),
                        queue_position: None,
                    },
                )
                .await
            }
            Err(error) => reject_handle(sender, request_id, error).await,
        },
        ClientCommand::Compact { instructions, .. } => {
            if *connection_kind != ConnectionKind::Attach {
                return reject(
                    sender,
                    request_id,
                    "invalid_command",
                    "context compaction is available only on an attached session".to_owned(),
                )
                .await;
            }
            match handle.compact_context(instructions).await {
                Ok(()) => {
                    send_json(
                        sender,
                        &ServerMessage::CommandAccepted {
                            request_id,
                            command: "compact",
                            run_id: None,
                            queue_position: None,
                        },
                    )
                    .await
                }
                Err(error) => reject_handle(sender, request_id, error).await,
            }
        }
        ClientCommand::SetModel { model, .. } => match handle.set_model(model).await {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "set_model",
                        run_id: None,
                        queue_position: None,
                    },
                )
                .await
            }
            Err(error) => reject_handle(sender, request_id, error).await,
        },
        ClientCommand::SetReasoningEffort { effort, .. } => {
            match handle.set_reasoning_effort(effort).await {
                Ok(()) => {
                    send_json(
                        sender,
                        &ServerMessage::CommandAccepted {
                            request_id,
                            command: "set_reasoning_effort",
                            run_id: None,
                            queue_position: None,
                        },
                    )
                    .await
                }
                Err(error) => reject_handle(sender, request_id, error).await,
            }
        }
        ClientCommand::SetCapabilityMode {
            capability_mode, ..
        } => match handle.set_capability_mode(capability_mode).await {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "set_capability_mode",
                        run_id: None,
                        queue_position: None,
                    },
                )
                .await
            }
            Err(error) => reject_handle(sender, request_id, error).await,
        },
        ClientCommand::AnswerAskUser {
            ask_id, answers, ..
        } => match handle.answer_ask_user(ask_id, answers).await {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "answer_askuser",
                        run_id: None,
                        queue_position: None,
                    },
                )
                .await
            }
            Err(error) => reject_handle(sender, request_id, error).await,
        },
        ClientCommand::Ping { .. } => send_json(sender, &ServerMessage::Pong { request_id }).await,
    }
}

fn prompt_title_content(content: &Content, skill: Option<&SkillInvocation>) -> Content {
    let Some(skill) = skill else {
        return content.clone();
    };
    let name = skill.name.trim().trim_start_matches('/');
    let arguments = skill
        .arguments
        .as_deref()
        .map(str::trim)
        .filter(|arguments| !arguments.is_empty())
        .or_else(|| match content {
            Content::Text(text) => {
                let text = text.trim();
                (!text.is_empty()).then_some(text)
            }
            Content::Parts(_) => None,
        });
    Content::text(match arguments {
        Some(arguments) => format!("/{name} {arguments}"),
        None => format!("/{name}"),
    })
}

async fn reject_handle(
    sender: &mut SplitSink<WebSocket, Message>,
    request_id: String,
    error: AgentHandleError,
) -> Result<(), ()> {
    let code = match error {
        AgentHandleError::QueueFull { .. } => "queue_full",
        AgentHandleError::Busy { .. } => "session_busy",
        AgentHandleError::NoActiveRun { .. } => "no_active_run",
        AgentHandleError::RunMismatch { .. } => "run_mismatch",
        AgentHandleError::InvalidCommand { .. } => "invalid_command",
        AgentHandleError::AskUserNotPending { .. } => "askuser_not_pending",
        AgentHandleError::InvalidAskUserAnswer { .. } => "invalid_askuser_answer",
        AgentHandleError::ActorStopped { .. } | AgentHandleError::ResponseDropped { .. } => {
            "actor_stopped"
        }
        AgentHandleError::Operation { .. } => "operation_failed",
    };
    reject(sender, request_id, code, error.to_string()).await
}

async fn reject(
    sender: &mut SplitSink<WebSocket, Message>,
    request_id: String,
    code: &'static str,
    message: String,
) -> Result<(), ()> {
    send_json(
        sender,
        &ServerMessage::CommandRejected {
            request_id,
            code,
            message,
        },
    )
    .await
}

async fn send_json(
    sender: &mut SplitSink<WebSocket, Message>,
    message: &ServerMessage,
) -> Result<(), ()> {
    let json = serde_json::to_string(message).map_err(|_| ())?;
    tokio::time::timeout(WS_WRITE_TIMEOUT, sender.send(Message::Text(json.into())))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_skill_titles_keep_the_user_facing_invocation() {
        let title = prompt_title_content(
            &Content::text("security"),
            Some(&SkillInvocation::new("review")),
        );
        assert_eq!(title.as_text(), Some("/review security"));
    }

    #[test]
    fn argument_free_skill_titles_are_not_empty() {
        let title =
            prompt_title_content(&Content::text(""), Some(&SkillInvocation::new("/review")));
        assert_eq!(title.as_text(), Some("/review"));
    }
}
