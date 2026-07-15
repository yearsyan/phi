use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, header},
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde::Deserialize;
use tokio::sync::broadcast;

use super::{
    AppState,
    auth::{self, WS_AUTH_PROTOCOL_PREFIX, WS_PROTOCOL},
    dto::{ClientCommand, ServerMessage, SessionConfigDto, SessionDto},
};
use crate::{
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
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSessionQuery {
    profile_id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachSessionQuery {}

async fn new_session(
    State(state): State<AppState>,
    Query(query): Query<NewSessionQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    if !authenticate_websocket(&headers, &state) {
        return auth::unauthorized_response();
    }
    let service = Arc::clone(state.service());
    let profile_id = query
        .profile_id
        .unwrap_or_else(|| DEFAULT_PROFILE_ID.to_owned());
    websocket
        .protocols([WS_PROTOCOL])
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_new(socket, service, profile_id))
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

async fn handle_new(socket: WebSocket, service: Arc<ApplicationService>, profile_id: String) {
    let (mut sender, receiver) = socket.split();
    if send_json(&mut sender, &ServerMessage::Building)
        .await
        .is_err()
    {
        return;
    }

    let prepared = match service.prepare_session(profile_id).await {
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
            mode: summary.mode,
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
            session: SessionDto::from(&snapshot),
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
        snapshot.last_event_sequence,
    )
    .await;
}

struct PendingActivation {
    service: Arc<ApplicationService>,
    prepared: PreparedSession,
    activated: bool,
}

async fn socket_loop(
    mut sender: SplitSink<WebSocket, Message>,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    mut events: broadcast::Receiver<RuntimeEvent>,
    handle: AgentHandle,
    mut pending: Option<PendingActivation>,
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
                        if handle_command(&mut sender, &handle, &mut pending, command)
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
                            session: SessionDto::from(&snapshot),
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
    command: ClientCommand,
) -> Result<(), ()> {
    let request_id = command.request_id().to_owned();
    match command {
        ClientCommand::Prompt { content, skill, .. } => {
            let content = match handle.prepare_prompt(content, skill.as_ref()) {
                Ok(content) => content,
                Err(error) => return reject_handle(sender, request_id, error).await,
            };
            let queued = if let Some(pending) = pending.as_mut()
                && !pending.activated
            {
                match pending
                    .service
                    .activate_and_enqueue(&pending.prepared, content)
                    .await
                {
                    Ok((_, queued)) => {
                        pending.activated = true;
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
        ClientCommand::SetMode { mode, .. } => match handle.set_mode(mode).await {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "set_mode",
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
        ClientCommand::DecidePlanApproval {
            approval_id,
            decision,
            ..
        } => match handle.decide_plan_approval(approval_id, decision).await {
            Ok(()) => {
                send_json(
                    sender,
                    &ServerMessage::CommandAccepted {
                        request_id,
                        command: "decide_plan_approval",
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
        AgentHandleError::PlanApprovalNotPending { .. } => "plan_approval_not_pending",
        AgentHandleError::InvalidPlanApprovalDecision { .. } => "invalid_plan_approval_decision",
        AgentHandleError::StalePlanApproval { .. } => "stale_plan_approval",
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
