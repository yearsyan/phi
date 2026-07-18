import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import {
  initialSessionState,
  type SessionAction,
  type SessionState,
  sessionReducer,
} from '../state/sessionReducer.ts';
import type {
  AskUserAnswer,
  CapabilityMode,
  ClientCommand,
  ReasoningEffort,
  ServerMessage,
  SkillInvocation,
} from '../types/wire.ts';
import type { SessionSocket } from '../ws/connection.ts';
import { attachSession, openNewSession } from '../ws/sessionConnection.ts';

export type SessionTarget =
  | {
      kind: 'new';
      profileId: string;
      agentProfileId?: string;
      capabilityMode?: CapabilityMode;
      workspace?: string;
      instanceId: number;
    }
  | { kind: 'attach'; sessionId: string };

export type ConnectionPhase =
  | 'idle'
  | 'connecting'
  | 'preparing'
  | 'reconnecting'
  | 'ready'
  | 'error';

export const SESSION_CONNECT_TIMEOUT_MS = 15_000;
export const SESSION_RECONNECT_DELAYS_MS = [800, 1_600, 3_200, 5_000] as const;

export interface DaemonSessionControls {
  state: SessionState;
  connectionPhase: ConnectionPhase;
  connectionError: string | null;
  sessionListRevision: number;
  retry: () => void;
  sendPrompt: (text: string, skill?: SkillInvocation) => boolean;
  stop: () => void;
  answerAsk: (askId: string, answers: AskUserAnswer[]) => boolean;
  setModel: (model: string) => void;
  setReasoningEffort: (effort: ReasoningEffort | null) => void;
  setCapabilityMode: (mode: CapabilityMode) => void;
  compact: (instructions?: string) => boolean;
  clearNotice: (index: number) => void;
}

let requestCounter = 0;
function nextRequestId(prefix: string): string {
  requestCounter += 1;
  return `${prefix}-${Date.now().toString(36)}-${requestCounter}`;
}

/**
 * Owns one durable browser-to-session relationship.
 *
 * A prepared session stays on its original socket after `session_created`.
 * Should that socket later drop, retries attach to the activated session id
 * rather than accidentally creating a second session.
 */
export function useDaemonSession(
  authKey: string,
  target: SessionTarget | null,
): DaemonSessionControls {
  const [state, dispatch] = useReducer(sessionReducer, initialSessionState);
  const [connectionPhase, setConnectionPhase] =
    useState<ConnectionPhase>('idle');
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const [sessionListRevision, setSessionListRevision] = useState(0);

  const socketRef = useRef<SessionSocket | null>(null);
  const promotedSessionIdRef = useRef<string | null>(null);
  const preparedPromptRequestIdRef = useRef<string | null>(null);
  const targetIdentityRef = useRef<string | null>(null);
  const lastSequenceRef = useRef(0);
  const reconnectCountRef = useRef(0);
  const reconnectTimerRef = useRef<number | null>(null);

  const cancelReconnect = useCallback(() => {
    if (reconnectTimerRef.current !== null) {
      window.clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
  }, []);

  const teardown = useCallback(() => {
    const socket = socketRef.current;
    socketRef.current = null;
    socket?.close();
  }, []);

  const send = useCallback((command: ClientCommand): boolean => {
    const socket = socketRef.current;
    if (socket === null || !socket.isOpen) {
      setConnectionPhase('error');
      setConnectionError('The session connection is not open.');
      return false;
    }
    try {
      socket.send(command);
      return true;
    } catch (error) {
      setConnectionPhase('error');
      setConnectionError(
        error instanceof Error ? error.message : String(error),
      );
      return false;
    }
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: connectionAttempt intentionally reopens the same logical target
  useEffect(() => {
    const targetIdentity =
      target === null
        ? null
        : target.kind === 'new'
          ? `new:${target.instanceId}:${target.profileId}:${target.agentProfileId ?? ''}:${target.capabilityMode ?? ''}:${target.workspace ?? ''}`
          : `attach:${target.sessionId}`;
    if (targetIdentityRef.current !== targetIdentity) {
      targetIdentityRef.current = targetIdentity;
      promotedSessionIdRef.current = null;
      preparedPromptRequestIdRef.current = null;
      lastSequenceRef.current = 0;
      reconnectCountRef.current = 0;
      cancelReconnect();
    }

    if (target === null) {
      teardown();
      dispatch({ type: 'reset' });
      setConnectionPhase('idle');
      setConnectionError(null);
      return;
    }

    if (!authKey.trim()) {
      teardown();
      dispatch({ type: 'reset' });
      setConnectionPhase('error');
      setConnectionError('Configure the daemon key before opening a session.');
      return;
    }

    let cancelled = false;
    let timedOut = false;
    let terminal = false;
    let reconnectScheduled = false;
    const controller = new AbortController();
    let deadline: number | null = null;

    const clearDeadline = () => {
      if (deadline !== null) {
        window.clearTimeout(deadline);
        deadline = null;
      }
    };

    const scheduleReconnect = (message: string) => {
      if (cancelled || terminal || reconnectScheduled) return;
      clearDeadline();
      teardown();
      dispatch({ type: 'disconnected' });

      if (
        target.kind === 'new' &&
        promotedSessionIdRef.current === null &&
        preparedPromptRequestIdRef.current !== null
      ) {
        terminal = true;
        setConnectionPhase('error');
        setConnectionError(
          `${message} The first prompt may already be running; the client will not resend it automatically.`,
        );
        return;
      }

      const delay =
        SESSION_RECONNECT_DELAYS_MS[reconnectCountRef.current] ?? null;
      if (delay === null) {
        setConnectionPhase('error');
        setConnectionError(message);
        return;
      }

      reconnectScheduled = true;
      reconnectCountRef.current += 1;
      setConnectionPhase('reconnecting');
      setConnectionError(message);
      reconnectTimerRef.current = window.setTimeout(() => {
        reconnectTimerRef.current = null;
        setConnectionAttempt((attempt) => attempt + 1);
      }, delay);
    };

    setConnectionPhase(
      reconnectCountRef.current > 0 ? 'reconnecting' : 'connecting',
    );
    setConnectionError(null);
    if (reconnectCountRef.current === 0) {
      dispatch({
        type: 'reset',
        profileId: target.kind === 'new' ? target.profileId : undefined,
      });
    }

    deadline = window.setTimeout(() => {
      if (cancelled) return;
      timedOut = true;
      const message = `Session connection timed out after ${SESSION_CONNECT_TIMEOUT_MS / 1000} seconds.`;
      controller.abort(new Error(message));
      scheduleReconnect(message);
    }, SESSION_CONNECT_TIMEOUT_MS);

    const handlers = {
      onMessage: (message: ServerMessage) => {
        if (cancelled) return;
        if (message.type === 'event') {
          if (message.sequence <= lastSequenceRef.current) return;
          if (message.sequence !== lastSequenceRef.current + 1) {
            scheduleReconnect(
              `Session events became out of sync at sequence ${message.sequence}.`,
            );
            return;
          }
          lastSequenceRef.current = message.sequence;
        } else if (
          message.type === 'snapshot' ||
          message.type === 'resync_required'
        ) {
          lastSequenceRef.current = message.session.last_sequence;
        }
        switch (message.type) {
          case 'building':
            setConnectionPhase('preparing');
            break;
          case 'session_created':
            promotedSessionIdRef.current = message.session_id;
            preparedPromptRequestIdRef.current = null;
            setSessionListRevision((revision) => revision + 1);
            break;
          case 'ready':
          case 'snapshot':
          case 'resync_required':
            clearDeadline();
            reconnectCountRef.current = 0;
            setConnectionPhase('ready');
            setConnectionError(null);
            break;
          case 'fatal_error':
            terminal = true;
            clearDeadline();
            setConnectionPhase('error');
            setConnectionError(message.message);
            break;
        }
        if (
          message.type === 'event' &&
          message.event.type === 'title_changed'
        ) {
          setSessionListRevision((revision) => revision + 1);
        }
        const action = serverMessageToAction(message);
        if (
          message.type === 'command_rejected' &&
          message.request_id === preparedPromptRequestIdRef.current
        ) {
          preparedPromptRequestIdRef.current = null;
        }
        if (action !== null) dispatch(action);
      },
      onClose: (event: CloseEvent) => {
        if (cancelled || terminal) return;
        socketRef.current = null;
        scheduleReconnect(
          event.reason || `Session connection closed (code ${event.code}).`,
        );
      },
      onError: (error: Error) => {
        if (cancelled || terminal) return;
        scheduleReconnect(error.message);
      },
    };

    (async () => {
      try {
        const promotedSessionId =
          target.kind === 'new' ? promotedSessionIdRef.current : null;
        const socket =
          promotedSessionId !== null
            ? await attachSession(authKey, promotedSessionId, handlers, {
                signal: controller.signal,
              })
            : target.kind === 'new'
              ? await openNewSession(authKey, target.profileId, handlers, {
                  signal: controller.signal,
                  agentProfileId: target.agentProfileId,
                  capabilityMode: target.capabilityMode,
                  workspace: target.workspace,
                })
              : await attachSession(authKey, target.sessionId, handlers, {
                  signal: controller.signal,
                });
        if (cancelled || terminal) {
          socket.close();
          return;
        }
        socketRef.current = socket;
        setConnectionPhase((current) =>
          current === 'connecting' || current === 'reconnecting'
            ? 'preparing'
            : current,
        );
      } catch (error) {
        if (cancelled || timedOut || reconnectScheduled) return;
        clearDeadline();
        terminal = true;
        dispatch({ type: 'disconnected' });
        setConnectionPhase('error');
        setConnectionError(
          error instanceof Error ? error.message : String(error),
        );
      }
    })();

    return () => {
      cancelled = true;
      clearDeadline();
      controller.abort();
      cancelReconnect();
      teardown();
    };
  }, [authKey, target, teardown, cancelReconnect, connectionAttempt]);

  const sendPrompt = useCallback(
    (text: string, skill?: SkillInvocation): boolean => {
      const trimmed = text.trim();
      const skillName = skill?.name.trim() ?? '';
      if (!trimmed && !skillName) return false;
      const requestId = nextRequestId('prompt');
      const content = { type: 'text' as const, value: trimmed };
      const sent = send({
        type: 'prompt',
        request_id: requestId,
        content,
        ...(skillName
          ? {
              skill: {
                name: skillName,
                arguments: skill?.arguments?.trim() || undefined,
              },
            }
          : {}),
      });
      if (sent) {
        if (target?.kind === 'new' && promotedSessionIdRef.current === null) {
          preparedPromptRequestIdRef.current = requestId;
        }
        const displayContent = skillName
          ? {
              type: 'text' as const,
              value: `/${skillName}${trimmed ? ` ${trimmed}` : ''}`,
            }
          : content;
        dispatch({
          type: 'local_send_prompt',
          requestId,
          content: displayContent,
          matchAnyEcho: Boolean(skillName),
        });
      }
      return sent;
    },
    [send, target],
  );

  const stop = useCallback(() => {
    const runId = state.activeRunId;
    if (runId === null) return;
    send({ type: 'stop', request_id: nextRequestId('stop'), run_id: runId });
  }, [send, state.activeRunId]);

  const answerAsk = useCallback(
    (askId: string, answers: AskUserAnswer[]): boolean =>
      send({
        type: 'answer_askuser',
        request_id: nextRequestId('answer'),
        ask_id: askId,
        answers,
      }),
    [send],
  );

  const setModel = useCallback(
    (model: string) => {
      const value = model.trim();
      if (!value) return;
      send({
        type: 'set_model',
        request_id: nextRequestId('model'),
        model: value,
      });
    },
    [send],
  );

  const setCapabilityMode = useCallback(
    (capabilityMode: CapabilityMode) => {
      send({
        type: 'set_capability_mode',
        request_id: nextRequestId('capability'),
        capability_mode: capabilityMode,
      });
    },
    [send],
  );

  const setReasoningEffort = useCallback(
    (effort: ReasoningEffort | null) => {
      send({
        type: 'set_reasoning_effort',
        request_id: nextRequestId('reasoning'),
        effort,
      });
    },
    [send],
  );

  const compact = useCallback(
    (instructions?: string): boolean =>
      send({
        type: 'compact',
        request_id: nextRequestId('compact'),
        instructions: instructions?.trim() || null,
      }),
    [send],
  );

  const retry = useCallback(() => {
    cancelReconnect();
    reconnectCountRef.current = 0;
    setConnectionAttempt((attempt) => attempt + 1);
  }, [cancelReconnect]);

  const clearNotice = useCallback((index: number) => {
    dispatch({ type: 'clear_notice', index });
  }, []);

  return {
    state,
    connectionPhase,
    connectionError,
    sessionListRevision,
    retry,
    sendPrompt,
    stop,
    answerAsk,
    setModel,
    setReasoningEffort,
    setCapabilityMode,
    compact,
    clearNotice,
  };
}

function serverMessageToAction(message: ServerMessage): SessionAction | null {
  switch (message.type) {
    case 'ready':
      return {
        type: 'ready',
        config: message.config,
        capabilityMode: message.capability_mode,
        agentProfile: message.agent_profile,
        workspace: message.workspace ?? null,
        skills: [...(message.skills ?? [])],
      };
    case 'session_created':
      return { type: 'session_created', sessionId: message.session_id };
    case 'snapshot':
      return { type: 'snapshot', session: message.session };
    case 'resync_required':
      return { type: 'resync', session: message.session };
    case 'fatal_error':
      return {
        type: 'fatal_error',
        code: message.code,
        message: message.message,
      };
    case 'event':
      return {
        type: 'event',
        envelope: {
          sequence: message.sequence,
          run_id: message.run_id,
          event: message.event,
        },
      };
    case 'command_accepted':
      return {
        type: 'command_accepted',
        requestId: message.request_id,
        queuePosition: message.queue_position ?? null,
      };
    case 'command_rejected':
      return {
        type: 'command_rejected',
        requestId: message.request_id,
        message: `${message.code}: ${message.message}`,
      };
    default:
      return null;
  }
}
