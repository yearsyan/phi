import type {
  AssistantDraft,
  ForkPosition,
  PublicMessage,
  ToolCall,
} from '../types/wire.ts';
import {
  type CompactionMarker,
  forkMessageIndex,
  type PendingPrompt,
  type RunActivity,
  type Step,
  type ToolStep,
} from './sessionReducer.ts';

/** Non-tool run events rendered as small gray status lines in the timeline. */
export type StatusStep = Exclude<Step, ToolStep>;

/**
 * Flat render unit of the chat timeline. Every row the list can show is one
 * item — user message, assistant text, a tool call/batch, or a status line —
 * so live activity and committed history share the same components.
 */
export interface ToolTimelineItem {
  kind: 'tool';
  key: string;
  call: ToolCall;
  status: 'running' | 'done' | 'error';
  progress: string[];
  output: string | null;
  /** Unparsed streamed arguments while the call is still being drafted. */
  streamingArgs: string | null;
}

export interface ToolBatchTimelineItem {
  kind: 'tool-batch';
  key: string;
  /** Calls emitted together by one assistant response, in provider order. */
  tools: ToolTimelineItem[];
}

export type TimelineItem =
  | {
      kind: 'user';
      key: string;
      message: PublicMessage;
      pending: PendingPrompt | null;
    }
  | {
      kind: 'assistant';
      key: string;
      /** Index in the provider-safe transcript; null until it is durable. */
      messageIndex: number | null;
      /** Whether the branch keeps this response or stops before its tools. */
      forkPosition: ForkPosition;
      reasoning: string;
      text: string;
      streaming: boolean;
    }
  | ToolTimelineItem
  | ToolBatchTimelineItem
  | {
      kind: 'status';
      key: string;
      step: StatusStep;
    }
  | {
      kind: 'compaction';
      key: string;
      phase: CompactionMarker['phase'];
      message?: string;
    };

export interface Timeline {
  items: TimelineItem[];
}

/**
 * Turns the provider-safe transcript plus the live run activity into a flat
 * chat timeline. Tool calls are first-class rows: when a call is covered by a
 * live run step, the step wins (progress, running state); otherwise the
 * committed tool result is used. Both cases render through the same row
 * component, so a resync no longer switches visuals mid-conversation.
 */
export function deriveTimeline(
  history: PublicMessage[],
  draft: AssistantDraft | null,
  pendingPrompts: PendingPrompt[] = [],
  activeRun: RunActivity | null = null,
  compactions: CompactionMarker[] = [],
): Timeline {
  const items: TimelineItem[] = [];
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

  const liveSteps = new Map<string, ToolStep>();
  if (activeRun !== null) {
    for (const turn of activeRun.turns) {
      for (const step of turn.steps) {
        if (step.kind === 'tool') liveSteps.set(step.key, step);
      }
    }
  }
  const draftStepKeys = new Set<string>();
  for (const call of draft?.tool_calls ?? []) {
    if (call.id !== null && call.name !== null) {
      draftStepKeys.add(`${call.name}:${call.id}`);
    }
  }

  const compactionsByIndex = new Map<number, CompactionMarker[]>();
  for (const compaction of compactions) {
    const historyIndex = Math.min(compaction.historyIndex, history.length);
    const atIndex = compactionsByIndex.get(historyIndex) ?? [];
    atIndex.push(compaction);
    compactionsByIndex.set(historyIndex, atIndex);
  }
  const appendCompactions = (historyIndex: number) => {
    for (const compaction of compactionsByIndex.get(historyIndex) ?? []) {
      items.push({
        kind: 'compaction',
        key: compaction.key,
        phase: compaction.phase,
        message: compaction.message,
      });
    }
  };

  for (let index = 0; index < history.length; index += 1) {
    appendCompactions(index);
    const message = history[index];
    if (message === undefined) continue;
    if (message.visibility === 'internal') continue;

    if (message.role === 'user') {
      items.push({
        kind: 'user',
        key: `user-${index}`,
        message,
        pending: null,
      });
      continue;
    }

    if (message.role === 'assistant') {
      const reasoning = message.reasoning?.trim() ?? '';
      const text = contentText(message);
      if (reasoning.length > 0 || text.length > 0) {
        items.push({
          kind: 'assistant',
          key: `assistant-${index}`,
          messageIndex: forkMessageIndex(index, compactions) ?? null,
          forkPosition:
            message.tool_calls.length > 0 ? 'before_tool_calls' : 'after',
          reasoning,
          text,
          streaming: false,
        });
      }

      if (message.tool_calls.length > 0) {
        const results = new Map<string, PublicMessage>();
        let cursor = index + 1;
        while (cursor < history.length && history[cursor]?.role === 'tool') {
          const result = history[cursor];
          if (result?.tool_call_id != null) {
            results.set(result.tool_call_id, result);
          }
          cursor += 1;
        }

        const inLiveRun = activeRun !== null && index >= activeRun.historyStart;
        appendToolBatch(
          items,
          message.tool_calls.map((call) => {
            const step = inLiveRun
              ? liveSteps.get(toolStepKey(call))
              : undefined;
            return toolItem(call, step, results.get(call.id));
          }),
        );
        index = cursor - 1;
      }
      continue;
    }

    // Orphaned tool results stay visible instead of silently disappearing.
    if (message.role === 'tool') {
      items.push({
        kind: 'tool',
        key: `orphan-${index}`,
        call: {
          id: message.tool_call_id ?? `orphan-${index}`,
          name: '',
          arguments: null,
        },
        status: message.tool_result_is_error ? 'error' : 'done',
        progress: [],
        output: contentText(message),
        streamingArgs: null,
      });
    }
  }
  appendCompactions(history.length);

  // Live run steps not yet covered by committed history: tools of the
  // in-flight turn and every run-level event, in recorded order.
  if (activeRun !== null) {
    const represented = new Set<string>();
    for (
      let index = activeRun.historyStart;
      index < history.length;
      index += 1
    ) {
      const message = history[index];
      if (message?.role === 'assistant') {
        for (const call of message.tool_calls) {
          represented.add(toolStepKey(call));
        }
      }
    }

    let statusCount = 0;
    for (const turn of activeRun.turns) {
      const unrepresentedTools = turn.steps
        .filter(
          (step): step is ToolStep =>
            step.kind === 'tool' &&
            !represented.has(step.key) &&
            !draftStepKeys.has(step.key),
        )
        .map((step) => toolItem(step.call, step, undefined));
      let appendedTools = false;
      for (const step of turn.steps) {
        if (step.kind === 'tool') {
          if (represented.has(step.key) || draftStepKeys.has(step.key)) {
            continue;
          }
          if (!appendedTools) {
            appendToolBatch(items, unrepresentedTools);
            appendedTools = true;
          }
        } else {
          items.push({
            kind: 'status',
            key: `status-${activeRun.runId}-${statusCount}`,
            step,
          });
          statusCount += 1;
        }
      }
    }

    if (activeRun.errorMessage !== null) {
      items.push({
        kind: 'status',
        key: `status-${activeRun.runId}-error`,
        step: {
          kind: 'notice',
          level: 'error',
          message: activeRun.errorMessage,
        },
      });
    }
  }

  if (
    draft !== null &&
    ((draft.reasoning?.length ?? 0) > 0 ||
      draft.text.length > 0 ||
      draft.tool_calls.length > 0)
  ) {
    if ((draft.reasoning?.length ?? 0) > 0 || draft.text.length > 0) {
      items.push({
        kind: 'assistant',
        key: 'draft-text',
        messageIndex: draft.fork_message_index ?? null,
        forkPosition:
          draft.tool_calls.length > 0 ? 'before_tool_calls' : 'after',
        reasoning: draft.reasoning ?? '',
        text: draft.text,
        streaming: true,
      });
    }
    appendToolBatch(
      items,
      draft.tool_calls.map((call): ToolTimelineItem => {
        const parsedCall: ToolCall = {
          id: call.id ?? `draft-${call.index}`,
          name: call.name ?? '',
          arguments: null,
        };
        const liveStep =
          call.id !== null && call.name !== null
            ? liveSteps.get(toolStepKey(parsedCall))
            : undefined;
        if (liveStep !== undefined) {
          return toolItem(parsedCall, liveStep, undefined);
        }
        return {
          kind: 'tool',
          key: `draft-tool-${call.id ?? call.index}`,
          call: parsedCall,
          status: 'running',
          progress: [],
          output: null,
          streamingArgs: call.arguments,
        };
      }),
    );
  } else if (activeRun?.status === 'running') {
    items.push({
      kind: 'assistant',
      key: 'pending-assistant',
      messageIndex: null,
      forkPosition: 'after',
      reasoning: '',
      text: '',
      streaming: true,
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

function toolItem(
  call: ToolCall,
  step: ToolStep | undefined,
  result: PublicMessage | undefined,
): ToolTimelineItem {
  if (step !== undefined) {
    return {
      kind: 'tool',
      key: `tool-${call.id}`,
      call: step.call,
      status:
        step.status === 'running' ? 'running' : step.isError ? 'error' : 'done',
      progress: step.progress,
      output: step.content,
      streamingArgs: null,
    };
  }
  return {
    kind: 'tool',
    key: `tool-${call.id}`,
    call,
    status: result?.tool_result_is_error ? 'error' : 'done',
    progress: [],
    output: result !== undefined ? contentText(result) : null,
    streamingArgs: null,
  };
}

/** Keep a single call byte-for-byte on the existing row path; only batches of
 * two or more calls gain the additional collapsible summary layer. */
function appendToolBatch(
  items: TimelineItem[],
  tools: ToolTimelineItem[],
): void {
  if (tools.length === 0) return;
  if (tools.length === 1) {
    const tool = tools[0];
    if (tool !== undefined) items.push(tool);
    return;
  }
  items.push({
    kind: 'tool-batch',
    key: `tool-batch:${tools[0]?.call.id ?? 'unknown'}`,
    tools,
  });
}

function toolStepKey(call: ToolCall): string {
  return `${call.name}:${call.id}`;
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
