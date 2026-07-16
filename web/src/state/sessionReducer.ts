/**
 * Pure projection of daemon `ServerMessage`s into client-side session state.
 *
 * The reducer is a plain function (no React) so it can be tested and driven by
 * any transport. It keeps two complementary pieces of state:
 *
 *   - `history`: the committed transcript (canonical from snapshot, appended to
 *     from turn/message events between snapshots).
 *   - `activeRun`: the ephemeral per-run activity log — tool calls, retries,
 *     compaction, subagent notices — grouped per turn so the UI can show a
 *     "work detail" block that expands while a turn runs and collapses when the
 *     turn ends (leaving only the assistant's final answer).
 *
 * On `snapshot` / `resync` the whole state is replaced with the server's
 * `SessionDto`; this is the documented contract for resync. Between resyncs,
 * events are applied incrementally with `lastSequence` monotonicity. All updates
 * are immutable.
 */
import type {
  AgentMode,
  AgentProfileRef,
  AskUserRequest,
  AssistantDelta,
  AssistantDraft,
  CapabilityMode,
  Content,
  ContextUsage,
  EventDto,
  PlanApprovalRequest,
  PublicMessage,
  RetryReason,
  SessionConfig,
  SessionDto,
  SessionStatus,
  SubagentSummary,
  TokenUsage,
  ToolCall,
  ToolCallDraft,
  ToolProgress,
  Usage,
} from '../types/wire.ts';

/* -------------------------------------------------------------------------- */
/* Step model — the contents of a "work detail" block                        */
/* -------------------------------------------------------------------------- */

export interface ToolStep {
  kind: 'tool';
  key: string;
  call: ToolCall;
  status: 'running' | 'done';
  progress: string[];
  content: string | null;
  isError: boolean;
}

export interface NoticeStep {
  kind: 'notice';
  level: 'info' | 'warn' | 'error';
  message: string;
}

export interface SubagentStep {
  kind: 'subagent';
  agentId: string;
  message: string;
  detail?: string;
}

export interface CompactionStep {
  kind: 'compaction';
  phase: 'started' | 'completed' | 'failed';
  message?: string;
}

export interface RetryStep {
  kind: 'retry';
  retryNumber: number;
  maxRetries: number;
  reason: string;
}

export type Step =
  | ToolStep
  | NoticeStep
  | SubagentStep
  | CompactionStep
  | RetryStep;

/** Per-turn activity log inside a run. */
export interface TurnActivity {
  turn: number;
  steps: Step[];
  /** Once the turn ends, the work detail collapses by default. */
  finished: boolean;
}

export interface RunActivity {
  runId: string;
  status: 'queued' | 'running' | 'completed' | 'stopped' | 'failed';
  turns: TurnActivity[];
  errorMessage: string | null;
  /** Transcript length when this run began, used to place live activity. */
  historyStart: number;
}

export interface PendingPrompt {
  requestId: string;
  content: Content;
  status: 'sending' | 'accepted' | 'queued';
  queuePosition: number | null;
}

/* -------------------------------------------------------------------------- */
/* Session state                                                              */
/* -------------------------------------------------------------------------- */

export interface SessionState {
  sessionId: string | null;
  profileId: string | null;
  agentProfile: AgentProfileRef | null;
  ready: boolean;
  status: SessionStatus;
  mode: AgentMode;
  capabilityMode: CapabilityMode;
  config: SessionConfig | null;
  usage: Usage | null;
  contextUsage: ContextUsage | null;
  activeRunId: string | null;
  queuedRuns: number;
  /** Live assistant draft (uncommitted). */
  draft: AssistantDraft | null;
  pendingAsks: AskUserRequest[];
  pendingPlanApprovals: PlanApprovalRequest[];
  subagents: SubagentSummary[];
  history: PublicMessage[];
  activeRun: RunActivity | null;
  /** Optimistic prompts waiting for their durable user-message echo. */
  pendingPrompts: PendingPrompt[];
  fatalError: { code: string; message: string } | null;
  notices: string[];
  lastSequence: number;
  /** Set when a gap was detected; the next snapshot/resync will recover. */
  resyncNeeded: boolean;
}

export const initialSessionState: SessionState = {
  sessionId: null,
  profileId: null,
  agentProfile: null,
  ready: false,
  status: 'awaiting_first_prompt',
  mode: 'default',
  capabilityMode: 'full_access',
  config: null,
  usage: null,
  contextUsage: null,
  activeRunId: null,
  queuedRuns: 0,
  draft: null,
  pendingAsks: [],
  pendingPlanApprovals: [],
  subagents: [],
  history: [],
  activeRun: null,
  pendingPrompts: [],
  fatalError: null,
  notices: [],
  lastSequence: 0,
  resyncNeeded: false,
};

/* -------------------------------------------------------------------------- */
/* Reducer input                                                              */
/* -------------------------------------------------------------------------- */

export type SessionAction =
  | { type: 'reset'; profileId?: string }
  | {
      type: 'ready';
      config: SessionConfig;
      mode: AgentMode;
      capabilityMode: CapabilityMode;
      agentProfile: AgentProfileRef;
    }
  | { type: 'session_created'; sessionId: string }
  | { type: 'snapshot'; session: SessionDto }
  | { type: 'resync'; session: SessionDto }
  | { type: 'disconnected' }
  | { type: 'event'; envelope: EventEnvelopeInput }
  | { type: 'fatal_error'; code: string; message: string }
  | { type: 'notice'; message: string }
  | { type: 'local_send_prompt'; requestId: string; content: Content }
  | {
      type: 'command_accepted';
      requestId: string;
      queuePosition: number | null;
    }
  | { type: 'command_rejected'; requestId: string; message: string }
  | { type: 'clear_notice'; index: number };

export interface EventEnvelopeInput {
  sequence: number;
  run_id?: string;
  event: EventDto;
}

/* -------------------------------------------------------------------------- */
/* Helpers (all immutable)                                                    */
/* -------------------------------------------------------------------------- */

function cloneDraft(draft: AssistantDraft | null): AssistantDraft | null {
  if (draft === null) return null;
  return {
    text: draft.text,
    tool_calls: draft.tool_calls.map((toolCall) => ({ ...toolCall })),
  };
}

function emptyDraft(): AssistantDraft {
  return { text: '', tool_calls: [] };
}

function fromSnapshot(session: SessionDto): SessionState {
  const activeRun: RunActivity | null =
    session.active_run_id === null
      ? null
      : {
          runId: session.active_run_id,
          status: 'running',
          turns: [],
          errorMessage: null,
          historyStart: session.history.length,
        };
  return {
    sessionId: session.session_id,
    profileId: session.profile_id,
    agentProfile: session.agent_profile,
    ready: true,
    status: session.status,
    mode: session.mode,
    capabilityMode: session.capability_mode,
    config: session.config,
    usage: session.usage,
    contextUsage: session.usage.context,
    activeRunId: session.active_run_id,
    queuedRuns: session.queued_runs,
    draft: cloneDraft(session.draft),
    pendingAsks: session.pending_asks,
    pendingPlanApprovals: session.pending_plan_approvals,
    subagents: session.subagents,
    history: session.history,
    activeRun,
    pendingPrompts: [],
    fatalError: null,
    notices: [],
    lastSequence: session.last_sequence,
    resyncNeeded: false,
  };
}

function toolStepKey(call: ToolCall): string {
  return `${call.name}:${call.id}`;
}

function retryReasonText(reason: RetryReason): string {
  switch (reason.type) {
    case 'request_timeout':
      return `request timeout (${reason.timeout_ms} ms)`;
    case 'transport':
      return `transport error: ${reason.message}`;
    case 'http_status':
      return `HTTP ${reason.status}`;
  }
}

function shallowEqualContent(a: Content | null, b: Content | null): boolean {
  if (a === null || b === null) return a === b;
  return JSON.stringify(a) === JSON.stringify(b);
}

/* -------------------------------------------------------------------------- */
/* Run / turn immutable helpers                                               */
/* -------------------------------------------------------------------------- */

function startRun(state: SessionState, runId: string): SessionState {
  return {
    ...state,
    activeRunId: runId,
    status: 'running',
    queuedRuns: Math.max(0, state.queuedRuns - 1),
    draft: null,
    activeRun: {
      runId,
      status: 'running',
      turns: [],
      errorMessage: null,
      historyStart: state.history.length,
    },
  };
}

/** Immutably update the current turn (creating it if needed) of the active run. */
function updateCurrentTurn(
  state: SessionState,
  runId: string | undefined,
  mutate: (
    turn: TurnActivity,
    turnNumber: number,
  ) => { turn: TurnActivity; turnNumber: number },
): SessionState {
  if (state.activeRun === null) return state;
  if (runId !== undefined && state.activeRun.runId !== runId) return state;
  const turns = [...state.activeRun.turns];
  let turnNumber: number;
  // Current unfinished turn, else the latest, else start at 1.
  const unfinishedIndex = turns.findIndex((turn) => !turn.finished);
  if (unfinishedIndex >= 0) {
    turnNumber = turns[unfinishedIndex].turn;
  } else if (turns.length > 0) {
    turnNumber = turns[turns.length - 1].turn;
  } else {
    turnNumber = 1;
  }
  const existingIndex = turns.findIndex((turn) => turn.turn === turnNumber);
  const base: TurnActivity =
    existingIndex >= 0
      ? {
          ...turns[existingIndex],
          steps: turns[existingIndex].steps.map((step) => ({ ...step })),
        }
      : { turn: turnNumber, steps: [], finished: false };
  const result = mutate(base, turnNumber);
  if (existingIndex >= 0) {
    turns[existingIndex] = result.turn;
  } else {
    turns.push(result.turn);
  }
  return { ...state, activeRun: { ...state.activeRun, turns } };
}

function ensureTurn(
  state: SessionState,
  runId: string | undefined,
  turnNumber: number,
): SessionState {
  if (state.activeRun === null) return state;
  if (runId !== undefined && state.activeRun.runId !== runId) return state;
  if (state.activeRun.turns.some((turn) => turn.turn === turnNumber)) {
    return state;
  }
  return {
    ...state,
    activeRun: {
      ...state.activeRun,
      turns: [
        ...state.activeRun.turns,
        { turn: turnNumber, steps: [], finished: false },
      ],
    },
  };
}

function recordToolStep(
  state: SessionState,
  runId: string | undefined,
  call: ToolCall,
  mutate: (step: ToolStep) => void,
): SessionState {
  const key = toolStepKey(call);
  return updateCurrentTurn(state, runId, (turn, turnNumber) => {
    const steps = [...turn.steps];
    const existingIndex = steps.findIndex(
      (entry): entry is ToolStep => entry.kind === 'tool' && entry.key === key,
    );
    const existing = existingIndex >= 0 ? steps[existingIndex] : undefined;
    let step: ToolStep;
    if (existing !== undefined && existing.kind === 'tool') {
      step = { ...existing, call }; // refresh streamed args
    } else {
      step = {
        kind: 'tool',
        key,
        call,
        status: 'running',
        progress: [],
        content: null,
        isError: false,
      };
    }
    mutate(step);
    if (existingIndex >= 0) {
      steps[existingIndex] = step;
    } else {
      steps.push(step);
    }
    return { turn: { ...turn, steps }, turnNumber };
  });
}

function pushStep(
  state: SessionState,
  runId: string | undefined,
  step: Step,
): SessionState {
  return updateCurrentTurn(state, runId, (turn, turnNumber) => ({
    turn: { ...turn, steps: [...turn.steps, step] },
    turnNumber,
  }));
}

function finalizeRun(
  state: SessionState,
  runId: string,
  status: 'completed' | 'stopped' | 'failed',
  message?: string,
): SessionState {
  if (
    state.activeRun === null ||
    state.activeRun.runId !== runId ||
    state.activeRunId !== runId
  ) {
    return {
      ...state,
      queuedRuns: Math.max(0, state.queuedRuns - 1),
    };
  }
  return {
    ...state,
    status: 'idle',
    activeRunId: null,
    draft: null,
    activeRun: {
      ...state.activeRun,
      status,
      errorMessage: message ?? null,
      turns: state.activeRun.turns.map((turn) => ({ ...turn, finished: true })),
    },
  };
}

/* -------------------------------------------------------------------------- */
/* Reducer                                                                    */
/* -------------------------------------------------------------------------- */

export function sessionReducer(
  state: SessionState,
  action: SessionAction,
): SessionState {
  switch (action.type) {
    case 'reset':
      return {
        ...initialSessionState,
        profileId: action.profileId ?? state.profileId,
      };

    case 'ready':
      return {
        ...state,
        ready: true,
        config: action.config,
        mode: action.mode,
        capabilityMode: action.capabilityMode,
        agentProfile: action.agentProfile,
      };

    case 'session_created':
      return { ...state, sessionId: action.sessionId };

    case 'snapshot':
    case 'resync':
      return fromSnapshot(action.session);

    case 'disconnected':
      return {
        ...state,
        ready: false,
        pendingPrompts: [],
      };

    case 'fatal_error':
      return {
        ...state,
        ready: false,
        status: 'offline',
        pendingPrompts: [],
        fatalError: { code: action.code, message: action.message },
      };

    case 'notice':
      return { ...state, notices: [...state.notices, action.message] };

    case 'local_send_prompt':
      return {
        ...state,
        pendingPrompts: [
          ...state.pendingPrompts,
          {
            requestId: action.requestId,
            content: action.content,
            status: 'sending',
            queuePosition: null,
          },
        ],
      };

    case 'command_accepted':
      return {
        ...state,
        pendingPrompts: state.pendingPrompts.map((prompt) =>
          prompt.requestId === action.requestId
            ? {
                ...prompt,
                status: action.queuePosition !== null ? 'queued' : 'accepted',
                queuePosition: action.queuePosition,
              }
            : prompt,
        ),
      };

    case 'command_rejected': {
      return {
        ...state,
        pendingPrompts: state.pendingPrompts.filter(
          (prompt) => prompt.requestId !== action.requestId,
        ),
        notices: [...state.notices, action.message],
      };
    }

    case 'clear_notice': {
      const notices = state.notices.slice();
      notices.splice(action.index, 1);
      return { ...state, notices };
    }

    case 'event': {
      const { sequence, run_id, event } = action.envelope;
      // Enforce monotonic sequence; on any gap, mark resync-needed. The server
      // delivers a fresh snapshot via resync_required when the client lags.
      if (sequence <= state.lastSequence) {
        return state;
      }
      if (state.resyncNeeded || sequence !== state.lastSequence + 1) {
        return { ...state, resyncNeeded: true };
      }
      const next = applyEvent(state, event, run_id);
      return { ...next, lastSequence: sequence };
    }

    default:
      return state;
  }
}

function applyEvent(
  state: SessionState,
  event: EventDto,
  runId: string | undefined,
): SessionState {
  switch (event.type) {
    case 'state_changed':
      return { ...state, status: event.status };

    case 'run_queued':
      return { ...state, queuedRuns: state.queuedRuns + 1 };

    case 'run_started':
      return startRun(state, event.run_id);

    case 'run_completed':
      return finalizeRun(state, event.run_id, 'completed');

    case 'run_stopped':
      return finalizeRun(state, event.run_id, 'stopped');

    case 'run_failed':
      return finalizeRun(state, event.run_id, 'failed', event.message);

    case 'config_changed':
      return { ...state, config: event.config };

    case 'mode_changed':
      return { ...state, mode: event.mode };

    case 'capability_mode_changed':
      return { ...state, capabilityMode: event.capability_mode };

    case 'askuser_requested':
      return { ...state, pendingAsks: [...state.pendingAsks, event.request] };

    case 'askuser_answered':
    case 'askuser_cancelled':
      return {
        ...state,
        pendingAsks: state.pendingAsks.filter(
          (ask) => ask.ask_id !== event.ask_id,
        ),
      };

    case 'plan_approval_requested':
      return {
        ...state,
        pendingPlanApprovals: [...state.pendingPlanApprovals, event.request],
      };

    case 'plan_approval_decided':
    case 'plan_approval_cancelled':
      return {
        ...state,
        pendingPlanApprovals: state.pendingPlanApprovals.filter(
          (approval) => approval.approval_id !== event.approval_id,
        ),
      };

    case 'operation_failed':
      return {
        ...state,
        notices: [...state.notices, `${event.operation}: ${event.message}`],
      };

    case 'actor_crashed':
      return {
        ...state,
        fatalError: { code: 'actor_crashed', message: event.message },
        status: 'idle',
      };

    case 'subagents_resynced':
      return { ...state, subagents: event.subagents };

    case 'agent_start':
    case 'agent_end':
    case 'agent_stopped':
    case 'session_initialized':
      return state;

    case 'message_start':
      return handleRoleMessage(state, event.message);

    case 'message_update': {
      const draft = state.draft === null ? emptyDraft() : state.draft;
      return { ...state, draft: applyDelta(draft, event.delta) };
    }

    case 'message_end':
      // User/tool messages are already projected at message_start or turn_end.
      // Assistant final content is committed atomically by turn_end.
      return state;

    case 'message_aborted':
      return { ...state, draft: null };

    case 'turn_start':
      return ensureTurn(state, runId, event.turn);

    case 'turn_end': {
      const afterTurn = ensureTurn(state, runId, event.turn);
      // Mark the turn finished (collapse the work detail).
      const markedTurn = updateCurrentTurn(
        afterTurn,
        runId,
        (turn, turnNumber) => ({
          turn: { ...turn, finished: true },
          turnNumber,
        }),
      );
      // Append the committed assistant message + tool results once.
      const history = [...state.history];
      if (event.message.role === 'assistant') {
        history.push(event.message);
      }
      for (const result of event.tool_results) {
        history.push(result);
      }
      return { ...markedTurn, history, draft: null };
    }

    case 'tool_execution_start':
      return recordToolStep(state, runId, event.call, (step) => {
        step.status = 'running';
        step.content = null;
        step.isError = false;
      });

    case 'tool_execution_progress':
      return recordToolStep(state, runId, event.call, (step) => {
        step.progress.push(event.progress.message);
      });

    case 'tool_execution_end':
      return recordToolStep(state, runId, event.call, (step) => {
        step.status = 'done';
        step.content = event.content;
        step.isError = event.is_error;
      });

    case 'subagent_spawned':
      return pushStep(state, runId, {
        kind: 'subagent',
        agentId: event.agent_id,
        message: `spawned subagent: ${event.description}`,
      });

    case 'subagent_state_changed':
      return pushStep(state, runId, {
        kind: 'subagent',
        agentId: event.agent_id,
        message: `subagent ${event.agent_id} → ${event.state}`,
      });

    case 'subagent_notification':
      return pushStep(state, runId, {
        kind: 'subagent',
        agentId: event.agent_id,
        message: event.notification.message,
        detail: `${event.notification.kind} (${event.notification.source})`,
      });

    case 'subagent_run_finished':
      return pushStep(state, runId, {
        kind: 'subagent',
        agentId: event.agent_id,
        message: `subagent run ${event.run_id} finished`,
      });

    case 'subagent_closed':
      return pushStep(state, runId, {
        kind: 'subagent',
        agentId: event.agent_id,
        message: `subagent closed: ${event.reason}`,
      });

    case 'subagent_message_queued':
    case 'subagent_agent_event':
      // Too noisy to surface individually; the parent observes via snapshots.
      return state;

    case 'provider_retry':
      return pushStep(state, runId, {
        kind: 'retry',
        retryNumber: event.retry_number,
        maxRetries: event.max_retries,
        reason: retryReasonText(event.reason),
      });

    case 'context_compaction_started':
      return pushStep(state, runId, {
        kind: 'compaction',
        phase: 'started',
        message: 'compacting context…',
      });

    case 'context_compaction_completed': {
      const withStep = pushStep(state, runId, {
        kind: 'compaction',
        phase: 'completed',
        message: `context compacted (${event.before_message_count} → ${event.after_message_count} messages)`,
      });
      const history =
        event.changed_from <= withStep.history.length
          ? [
              ...withStep.history.slice(0, event.changed_from),
              ...event.replacement,
            ]
          : withStep.history;
      return {
        ...withStep,
        history,
        draft: null,
        usage: withStep.usage
          ? {
              ...withStep.usage,
              last: null,
              context: null,
              cumulative: event.usage
                ? addUsage(withStep.usage.cumulative, event.usage)
                : withStep.usage.cumulative,
            }
          : null,
        contextUsage: null,
      };
    }

    case 'context_compaction_failed':
      return pushStep(state, runId, {
        kind: 'compaction',
        phase: 'failed',
        message: `compaction failed: ${event.message}`,
      });

    case 'usage_update':
      return {
        ...state,
        usage: {
          last: event.usage,
          context: event.context_usage,
          cumulative: addUsage(
            state.usage?.cumulative ?? emptyUsage(),
            event.usage,
          ),
        },
        contextUsage: event.context_usage,
      };

    case 'error':
      return { ...state, notices: [...state.notices, event.message] };

    default:
      return state;
  }
}

function handleRoleMessage(
  state: SessionState,
  message: PublicMessage,
): SessionState {
  if (message.role === 'user') {
    // The server echoes each committed user prompt. Remove exactly one matching
    // optimistic entry so identical queued prompts remain distinct.
    const pendingIndex = state.pendingPrompts.findIndex((prompt) =>
      shallowEqualContent(prompt.content, message.content),
    );
    const pendingPrompts =
      pendingIndex < 0
        ? state.pendingPrompts
        : state.pendingPrompts.filter((_, index) => index !== pendingIndex);
    const last = state.history[state.history.length - 1];
    const duplicateFrame =
      pendingIndex < 0 &&
      last !== undefined &&
      last.role === 'user' &&
      shallowEqualContent(last.content, message.content);
    const history = duplicateFrame
      ? state.history
      : [...state.history, message];
    return { ...state, history, pendingPrompts };
  }
  if (message.role === 'assistant') {
    // Begin a fresh assistant draft (text/tool_calls stream via message_update).
    return { ...state, draft: emptyDraft() };
  }
  return state;
}

function applyDelta(
  draft: AssistantDraft,
  delta: AssistantDelta,
): AssistantDraft {
  if (delta.type === 'text') {
    return { ...draft, text: draft.text + delta.delta };
  }
  const toolCalls = draft.tool_calls.map((toolCall) => ({ ...toolCall }));
  const existing = toolCalls.find((toolCall) => toolCall.index === delta.index);
  if (existing) {
    if (delta.id !== null) existing.id = delta.id;
    if (delta.name !== null) existing.name = delta.name;
    existing.arguments += delta.arguments_delta;
  } else {
    const newToolCall: ToolCallDraft = {
      index: delta.index,
      id: delta.id,
      name: delta.name,
      arguments: delta.arguments_delta,
    };
    toolCalls.push(newToolCall);
    toolCalls.sort((a, b) => a.index - b.index);
  }
  return { ...draft, tool_calls: toolCalls };
}

function emptyUsage(): TokenUsage {
  return {
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    cached_input_tokens: 0,
  };
}

function addUsage(left: TokenUsage, right: TokenUsage): TokenUsage {
  return {
    input_tokens: left.input_tokens + right.input_tokens,
    output_tokens: left.output_tokens + right.output_tokens,
    total_tokens: left.total_tokens + right.total_tokens,
    cached_input_tokens: left.cached_input_tokens + right.cached_input_tokens,
  };
}

// Re-exported for tests / type narrowing.
export type { ToolProgress };
