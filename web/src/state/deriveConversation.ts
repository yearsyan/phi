import type { AssistantDraft, PublicMessage, ToolCall } from '../types/wire.ts';
import type { PendingPrompt, RunActivity } from './sessionReducer.ts';

export type ConversationItem =
  | {
      kind: 'user';
      key: string;
      message: PublicMessage;
      pending: PendingPrompt | null;
    }
  | {
      kind: 'assistant';
      key: string;
      message: PublicMessage | null;
      draft: AssistantDraft | null;
      pending: boolean;
    }
  | {
      kind: 'activity';
      key: string;
      run: RunActivity;
    }
  | {
      kind: 'toolGroup';
      key: string;
      calls: ToolCall[];
      results: PublicMessage[];
    };

export interface DerivedConversation {
  items: ConversationItem[];
}

/**
 * Turns the provider-safe transcript into a human chat timeline.
 *
 * Protocol-only assistant tool-call messages and their tool results are grouped
 * into one compact item. Live activity is inserted before the current run's
 * final answer, instead of producing empty assistant rows after the answer.
 */
export function deriveConversation(
  history: PublicMessage[],
  draft: AssistantDraft | null,
  pendingPrompts: PendingPrompt[] = [],
  activeRun: RunActivity | null = null,
): DerivedConversation {
  const items: ConversationItem[] = [];
  let pendingStart = 0;
  const pendingCurrentPrompt =
    activeRun !== null &&
    history.length === activeRun.historyStart &&
    pendingPrompts[0] !== undefined
      ? pendingPrompts[0]
      : null;
  if (pendingCurrentPrompt !== null) {
    items.push({
      kind: 'user',
      key: `pending-${pendingCurrentPrompt.requestId}`,
      message: pendingMessage(pendingCurrentPrompt),
      pending: pendingCurrentPrompt,
    });
    pendingStart = 1;
  }

  const showActivity =
    activeRun !== null &&
    (activeRun.status === 'running' ||
      activeRun.status === 'queued' ||
      activeRun.status === 'failed' ||
      activeRun.status === 'stopped' ||
      activeRun.turns.some((turn) => turn.steps.length > 0));
  let activityInserted = false;

  const insertActivity = () => {
    if (!showActivity || activeRun === null || activityInserted) return;
    items.push({
      kind: 'activity',
      key: `activity-${activeRun.runId}`,
      run: activeRun,
    });
    activityInserted = true;
  };

  for (let index = 0; index < history.length; index += 1) {
    const message = history[index];
    if (message === undefined) continue;

    if (
      activeRun !== null &&
      index >= activeRun.historyStart &&
      message.role !== 'user'
    ) {
      insertActivity();
    }

    if (message.role === 'user') {
      items.push({
        kind: 'user',
        key: `history-user-${index}`,
        message,
        pending: null,
      });
      continue;
    }

    if (message.role === 'assistant') {
      const text = contentText(message);
      if (text.length > 0) {
        items.push({
          kind: 'assistant',
          key: `history-assistant-${index}`,
          message,
          draft: null,
          pending: false,
        });
      }

      if (message.tool_calls.length > 0) {
        const results: PublicMessage[] = [];
        let cursor = index + 1;
        while (cursor < history.length && history[cursor]?.role === 'tool') {
          const result = history[cursor];
          if (result !== undefined) results.push(result);
          cursor += 1;
        }

        const coveredByLiveActivity =
          activeRun !== null && index >= activeRun.historyStart;
        if (!coveredByLiveActivity) {
          items.push({
            kind: 'toolGroup',
            key: `history-tools-${index}`,
            calls: message.tool_calls,
            results,
          });
        }
        index = cursor - 1;
      }
      continue;
    }

    // Orphaned tool results should be visible instead of silently disappearing.
    if (message.role === 'tool') {
      items.push({
        kind: 'toolGroup',
        key: `history-tool-${index}`,
        calls: [],
        results: [message],
      });
    }
  }

  insertActivity();

  if (
    draft !== null &&
    (draft.text.length > 0 || draft.tool_calls.length > 0)
  ) {
    items.push({
      kind: 'assistant',
      key: 'streaming-assistant',
      message: null,
      draft,
      pending: true,
    });
  } else if (activeRun?.status === 'running') {
    items.push({
      kind: 'assistant',
      key: 'pending-assistant',
      message: null,
      draft: null,
      pending: true,
    });
  }

  for (const prompt of pendingPrompts.slice(pendingStart)) {
    items.push({
      kind: 'user',
      key: `pending-${prompt.requestId}`,
      message: pendingMessage(prompt),
      pending: prompt,
    });
  }

  return { items };
}

function pendingMessage(prompt: PendingPrompt): PublicMessage {
  return {
    role: 'user',
    content: prompt.content,
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function contentText(message: PublicMessage): string {
  if (message.content === null) return '';
  if (message.content.type === 'text') return message.content.value.trim();
  return message.content.value
    .filter((part) => part.type === 'text')
    .map((part) => part.text)
    .join('')
    .trim();
}
