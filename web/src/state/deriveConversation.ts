/**
 * Derive an ordered list of conversation items for the chat view.
 *
 * The reducer owns the committed `history` (durable transcript) and the
 * ephemeral `activeRun` (the current run's per-turn activity log). This pure
 * function interleaves them into a flat, renderable sequence:
 *
 *   1. Each committed transcript message is rendered as final content only —
 *      assistant answers appear as plain Markdown, tool results as compact
 *      summaries. Past turns deliberately carry no "work detail": the UX is that
 *      a turn's expanded steps are a live affordance, collapsed to the final
 *      answer once the turn completes.
 *   2. The current run's turns (which carry the live tool/retry/subagent step
 *      logs) are appended as a trailing activity segment. Each turn's steps
 *      expand while the turn runs and collapse (by default) once it finishes.
 *   3. A still-streaming assistant draft with no committed message yet is
 *      rendered as the final assistant item.
 *
 * This split keeps the durable transcript stable across runs/reconnects while
 * the ephemeral step logs live only with the run that produced them — avoiding
 * any cross-run position pairing between history messages and run turns.
 */
import type { AssistantDraft, Content, PublicMessage } from '../types/wire.ts';
import type { RunActivity, Step, TurnActivity } from './sessionReducer.ts';

export type ConversationItem =
  | {
      kind: 'user';
      key: string;
      message: PublicMessage;
      /** True for the optimistic bubble shown before the server echoes. */
      optimistic: boolean;
    }
  | {
      kind: 'assistant';
      key: string;
      message: PublicMessage | null;
      draft: AssistantDraft | null;
      turnNumber: number | null;
      steps: Step[];
      /** Collapsed (turn finished) vs expanded (turn running). */
      collapsed: boolean;
      failed: boolean;
      errorMessage: string | null;
      runStatus: RunActivity['status'] | null;
    }
  | { kind: 'toolResult'; key: string; message: PublicMessage };

export interface DerivedConversation {
  items: ConversationItem[];
}

/**
 * @param history       committed transcript
 * @param draft         live assistant draft (uncommitted)
 * @param pendingUser   optimistic user content not yet echoed by the server
 * @param activeRun     ephemeral run activity log (may be null when idle)
 */
export function deriveConversation(
  history: PublicMessage[],
  draft: AssistantDraft | null,
  pendingUser: Content | null,
  activeRun: RunActivity | null,
): DerivedConversation {
  const items: ConversationItem[] = [];

  // (1) Committed history → final content only.
  for (let i = 0; i < history.length; i += 1) {
    const message = history[i];
    if (message === undefined) continue;

    if (message.role === 'user') {
      items.push({
        kind: 'user',
        key: `hist-${i}`,
        message,
        optimistic: false,
      });
      continue;
    }
    if (message.role === 'assistant') {
      items.push({
        kind: 'assistant',
        key: `hist-${i}`,
        message,
        draft: null,
        turnNumber: null,
        steps: [],
        collapsed: true,
        failed: false,
        errorMessage: null,
        runStatus: null,
      });
      continue;
    }
    if (message.role === 'tool') {
      // Compact, collapsed tool-result summary (part of a past turn's detail).
      items.push({ kind: 'toolResult', key: `hist-tool-${i}`, message });
    }
  }

  // (2) Trailing segment from the live run: per-turn step logs + streaming draft.
  if (activeRun !== null) {
    const hasStreamingDraft =
      draft !== null && (draft.text.length > 0 || draft.tool_calls.length > 0);

    for (const turn of activeRun.turns) {
      // The last unfinished turn's streaming draft is folded into its item below
      // to avoid rendering two assistant blocks for the same in-flight turn.
      const isStreamingTurn =
        !turn.finished && turn === lastUnfinished(activeRun);
      if (isStreamingTurn && (hasStreamingDraft || turn.steps.length === 0)) {
        items.push(buildStreamingItem(activeRun, turn, draft));
        continue;
      }
      items.push(buildTurnItem(activeRun, turn, null));
    }

    // A streaming draft whose turn isn't represented yet (e.g. draft arrived
    // before turn_start, or run has no turns).
    const unfinished = lastUnfinished(activeRun);
    if (
      hasStreamingDraft &&
      (unfinished === null || !activeRun.turns.includes(unfinished))
    ) {
      items.push(buildStreamingItem(activeRun, unfinished, draft));
    }
  } else if (
    draft !== null &&
    (draft.text.length > 0 || draft.tool_calls.length > 0)
  ) {
    // Draft with no known run (rare, e.g. during resync): render it standalone.
    items.push({
      kind: 'assistant',
      key: 'streaming',
      message: null,
      draft,
      turnNumber: null,
      steps: [],
      collapsed: false,
      failed: false,
      errorMessage: null,
      runStatus: null,
    });
  }

  // (3) Optimistic user bubble (sent, not yet echoed by the server).
  if (pendingUser !== null && !historyEchoes(history, pendingUser)) {
    items.push({
      kind: 'user',
      key: 'pending-user',
      message: {
        role: 'user',
        content: pendingUser,
        tool_calls: [],
        tool_call_id: null,
        tool_result_is_error: false,
      },
      optimistic: true,
    });
  }

  return { items };
}

function lastUnfinished(run: RunActivity): TurnActivity | null {
  return run.turns.find((turn) => !turn.finished) ?? null;
}

function buildTurnItem(
  run: RunActivity,
  turn: TurnActivity,
  draft: AssistantDraft | null,
): ConversationItem {
  return {
    kind: 'assistant',
    key: `run-turn-${turn.turn}`,
    message: null,
    draft,
    turnNumber: turn.turn,
    steps: turn.steps,
    collapsed: turn.finished,
    failed: run.status === 'failed' && turn === run.turns[run.turns.length - 1],
    errorMessage: run.errorMessage,
    runStatus: run.status,
  };
}

function buildStreamingItem(
  run: RunActivity,
  turn: TurnActivity | null,
  draft: AssistantDraft | null,
): ConversationItem {
  return {
    kind: 'assistant',
    key: 'streaming',
    message: null,
    draft,
    turnNumber: turn?.turn ?? null,
    steps: turn?.steps ?? [],
    collapsed: false,
    failed: run.status === 'failed',
    errorMessage: run.errorMessage,
    runStatus: run.status,
  };
}

function historyEchoes(history: PublicMessage[], content: Content): boolean {
  const serialized = JSON.stringify(content);
  return history.some(
    (message) =>
      message.role === 'user' &&
      message.content !== null &&
      JSON.stringify(message.content) === serialized,
  );
}
