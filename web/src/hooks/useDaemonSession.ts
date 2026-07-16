import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import {
  initialSessionState,
  type SessionAction,
  type SessionState,
  sessionReducer,
} from '../state/sessionReducer.ts';
import type {
  AgentMode,
  AskUserAnswer,
  ClientCommand,
  PlanApprovalDecision,
  ServerMessage,
} from '../types/wire.ts';
import type { SessionSocket } from '../ws/connection.ts';
import { attachSession, openNewSession } from '../ws/sessionConnection.ts';

/** What kind of session to open. */
export type SessionTarget =
  | { kind: 'new'; profileId: string }
  | { kind: 'attach'; sessionId: string };

export type ConnectionPhase =
  | 'idle'
  | 'connecting'
  | 'preparing'
  | 'ready'
  | 'error';

export const SESSION_CONNECT_TIMEOUT_MS = 15_000;

export interface DaemonSessionControls {
  state: SessionState;
  /** Connection lifecycle for the current target. */
  connectionPhase: ConnectionPhase;
  connectionError: string | null;
  retry: () => void;
  /** Send a text prompt. */
  sendPrompt: (text: string) => void;
  /** Stop the active run (no-op if none). */
  stop: () => void;
  answerAsk: (askId: string, answers: AskUserAnswer[]) => void;
  decidePlan: (approvalId: string, decision: PlanApprovalDecision) => void;
  setModel: (model: string) => void;
  setMode: (mode: AgentMode) => void;
  compact: (instructions?: string) => void;
}

let requestCounter = 0;
function nextRequestId(prefix: string): string {
  requestCounter += 1;
  return `${prefix}-${Date.now().toString(36)}-${requestCounter}`;
}

/**
 * Owns the WebSocket lifecycle and the projection for a single session target.
 *
 * Opening a new target closes any existing socket and resets the reducer. On
 * unexpected close, the connection is surfaced via `connectionError` and can
 * be retried without changing the target.
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

  const socketRef = useRef<SessionSocket | null>(null);

  const teardown = useCallback(() => {
    const socket = socketRef.current;
    if (socket !== null) {
      socket.close();
      socketRef.current = null;
    }
  }, []);

  const send = useCallback((command: ClientCommand): boolean => {
    const socket = socketRef.current;
    if (socket === null || !socket.isOpen) {
      setConnectionPhase('error');
      setConnectionError('WebSocket is not open');
      dispatch({ type: 'disconnected' });
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
      dispatch({ type: 'disconnected' });
      return false;
    }
  }, []);

  // Open the socket whenever the target changes.
  // biome-ignore lint/correctness/useExhaustiveDependencies: connectionAttempt intentionally restarts the same target
  useEffect(() => {
    if (target === null) {
      teardown();
      dispatch({ type: 'reset' });
      setConnectionPhase('idle');
      setConnectionError(null);
      return;
    }

    if (!authKey) {
      teardown();
      dispatch({ type: 'reset' });
      setConnectionPhase('error');
      setConnectionError(
        'Missing daemon auth key. Open Settings to configure it.',
      );
      return;
    }

    let cancelled = false;
    let timedOut = false;
    const controller = new AbortController();
    let deadline: number | null = null;
    const clearDeadline = () => {
      if (deadline !== null) {
        window.clearTimeout(deadline);
        deadline = null;
      }
    };

    setConnectionPhase('connecting');
    setConnectionError(null);
    dispatch({
      type: 'reset',
      profileId: target.kind === 'new' ? target.profileId : undefined,
    });

    deadline = window.setTimeout(() => {
      if (cancelled) return;
      timedOut = true;
      const error = new Error(
        `Session connection timed out after ${SESSION_CONNECT_TIMEOUT_MS / 1000} seconds`,
      );
      controller.abort(error);
      teardown();
      dispatch({ type: 'disconnected' });
      setConnectionPhase('error');
      setConnectionError(error.message);
    }, SESSION_CONNECT_TIMEOUT_MS);

    const handlers = {
      onMessage: (message: ServerMessage) => {
        if (cancelled) return;
        switch (message.type) {
          case 'building':
            setConnectionPhase('preparing');
            break;
          case 'ready':
          case 'snapshot':
          case 'resync_required':
            clearDeadline();
            setConnectionPhase('ready');
            setConnectionError(null);
            break;
          case 'fatal_error':
            clearDeadline();
            setConnectionPhase('error');
            setConnectionError(message.message);
            break;
        }
        const action = serverMessageToAction(message);
        if (action !== null) {
          dispatch(action);
        }
      },
      onClose: (event: CloseEvent) => {
        if (cancelled) return;
        clearDeadline();
        socketRef.current = null;
        dispatch({ type: 'disconnected' });
        setConnectionPhase('error');
        setConnectionError(
          (current) =>
            current || event.reason || `WebSocket closed (code ${event.code})`,
        );
      },
      onError: (error: Error) => {
        if (cancelled) return;
        clearDeadline();
        teardown();
        dispatch({ type: 'disconnected' });
        setConnectionPhase('error');
        setConnectionError(error.message);
      },
    };

    (async () => {
      try {
        const socket =
          target.kind === 'new'
            ? await openNewSession(authKey, target.profileId, handlers, {
                signal: controller.signal,
              })
            : await attachSession(authKey, target.sessionId, handlers, {
                signal: controller.signal,
              });
        if (cancelled) {
          socket.close();
          return;
        }
        socketRef.current = socket;
        setConnectionPhase((current) =>
          current === 'connecting' ? 'preparing' : current,
        );
      } catch (error) {
        if (cancelled || timedOut) return;
        clearDeadline();
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
      teardown();
    };
  }, [authKey, target, teardown, connectionAttempt]);

  const sendPrompt = useCallback(
    (text: string) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      const content = { type: 'text' as const, value: trimmed };
      const sent = send({
        type: 'prompt',
        request_id: nextRequestId('prompt'),
        content,
      });
      if (sent) {
        dispatch({ type: 'local_send_prompt', content });
      }
    },
    [send],
  );

  const stop = useCallback(() => {
    const runId = state.activeRunId;
    if (runId === null) return;
    send({ type: 'stop', request_id: nextRequestId('stop'), run_id: runId });
  }, [send, state.activeRunId]);

  const answerAsk = useCallback(
    (askId: string, answers: AskUserAnswer[]) => {
      send({
        type: 'answer_askuser',
        request_id: nextRequestId('answer'),
        ask_id: askId,
        answers,
      });
    },
    [send],
  );

  const decidePlan = useCallback(
    (approvalId: string, decision: PlanApprovalDecision) => {
      send({
        type: 'decide_plan_approval',
        request_id: nextRequestId('plan'),
        approval_id: approvalId,
        decision,
      });
    },
    [send],
  );

  const setModel = useCallback(
    (model: string) => {
      if (!model.trim()) return;
      send({ type: 'set_model', request_id: nextRequestId('model'), model });
    },
    [send],
  );

  const setMode = useCallback(
    (mode: AgentMode) => {
      send({ type: 'set_mode', request_id: nextRequestId('mode'), mode });
    },
    [send],
  );

  const compact = useCallback(
    (instructions?: string) => {
      send({
        type: 'compact',
        request_id: nextRequestId('compact'),
        instructions: instructions ?? null,
      });
    },
    [send],
  );

  const retry = useCallback(() => {
    setConnectionAttempt((attempt) => attempt + 1);
  }, []);

  return {
    state,
    connectionPhase,
    connectionError,
    retry,
    sendPrompt,
    stop,
    answerAsk,
    decidePlan,
    setModel,
    setMode,
    compact,
  };
}

/**
 * Translate a server frame into a reducer action, or `null` when the frame
 * does not affect session state (control frames, direct command responses).
 */
function serverMessageToAction(message: ServerMessage): SessionAction | null {
  switch (message.type) {
    case 'ready':
      return { type: 'ready', config: message.config, mode: message.mode };
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
    case 'command_rejected':
      // Surface the rejection as a transient notice, not a blocking error.
      return { type: 'notice', message: `${message.code}: ${message.message}` };
    // Control frames and direct command responses carry no session-state change.
    default:
      return null;
  }
}
