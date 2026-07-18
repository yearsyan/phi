import { describe, expect, it } from 'vitest';
import type { PublicMessage, ToolCall } from '../types/wire.ts';
import type {
  CompactionMarker,
  RunActivity,
  ToolStep,
} from './sessionReducer.ts';
import { deriveTimeline } from './timeline.ts';

function user(text: string): PublicMessage {
  return {
    role: 'user',
    content: { type: 'text', value: text },
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function assistant(text: string, toolCalls: ToolCall[] = []): PublicMessage {
  return {
    role: 'assistant',
    content: text.length > 0 ? { type: 'text', value: text } : null,
    tool_calls: toolCalls,
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function toolResult(
  callId: string,
  text: string,
  isError = false,
): PublicMessage {
  return {
    role: 'tool',
    content: { type: 'text', value: text },
    tool_calls: [],
    tool_call_id: callId,
    tool_result_is_error: isError,
  };
}

function call(id: string, name: string): ToolCall {
  return { id, name, arguments: { path: '/tmp/x' } };
}

function toolStep(
  stepCall: ToolCall,
  overrides: Partial<ToolStep> = {},
): ToolStep {
  return {
    kind: 'tool',
    key: `${stepCall.name}:${stepCall.id}`,
    call: stepCall,
    status: 'done',
    progress: [],
    content: 'live output',
    isError: false,
    ...overrides,
  };
}

describe('deriveTimeline', () => {
  it('groups multiple tool calls from one assistant response', () => {
    const timeline = deriveTimeline(
      [
        user('fix it'),
        assistant('', [call('c1', 'read'), call('c2', 'shell')]),
        toolResult('c1', 'file body'),
        toolResult('c2', 'boom', true),
        assistant('done'),
      ],
      null,
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'tool-batch',
      'assistant',
    ]);
    const batch = timeline.items[1];
    expect(batch).toMatchObject({
      kind: 'tool-batch',
      key: 'tool-batch:c1',
    });
    if (batch?.kind !== 'tool-batch') throw new Error('expected tool batch');
    const [first, second] = batch.tools;
    expect(first).toMatchObject({
      key: 'tool-c1',
      status: 'done',
      output: 'file body',
    });
    expect(second).toMatchObject({ key: 'tool-c2', status: 'error' });
  });

  it('keeps a single assistant tool call on the existing tool row path', () => {
    const timeline = deriveTimeline(
      [
        user('inspect it'),
        assistant('', [call('c1', 'read')]),
        toolResult('c1', 'file body'),
      ],
      null,
    );

    expect(timeline.items.map((item) => item.kind)).toEqual(['user', 'tool']);
    expect(timeline.items[1]).toMatchObject({
      kind: 'tool',
      key: 'tool-c1',
      output: 'file body',
    });
  });

  it('keeps orphaned tool results visible', () => {
    const timeline = deriveTimeline([toolResult('c9', 'leftover')], null);

    expect(timeline.items).toHaveLength(1);
    expect(timeline.items[0]).toMatchObject({
      kind: 'tool',
      status: 'done',
      output: 'leftover',
    });
  });

  it('keeps internal runtime prompts out of the visible timeline', () => {
    const internal = {
      ...user('<subagent_notification>{...}</subagent_notification>'),
      visibility: 'internal' as const,
    };

    const timeline = deriveTimeline(
      [
        user('delegate this'),
        assistant('working'),
        internal,
        assistant('done'),
      ],
      null,
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'assistant',
      'assistant',
    ]);
    expect(
      timeline.items.some(
        (item) =>
          item.kind === 'user' && item.message.visibility === 'internal',
      ),
    ).toBe(false);
  });

  it('prefers live run steps over committed results for calls in the active run', () => {
    const read = call('c1', 'read');
    const run: RunActivity = {
      runId: 'run-1',
      status: 'running',
      turns: [
        {
          turn: 1,
          finished: false,
          steps: [toolStep(read, { status: 'running', progress: ['reading'] })],
        },
      ],
      errorMessage: null,
      historyStart: 0,
    };
    const timeline = deriveTimeline(
      [user('go'), assistant('', [read]), toolResult('c1', 'committed')],
      null,
      [],
      run,
    );

    const tool = timeline.items.find((item) => item.kind === 'tool');
    expect(tool).toMatchObject({
      status: 'running',
      progress: ['reading'],
      output: 'live output',
    });
    // The live step was consumed by the history walk, not appended again.
    expect(timeline.items.filter((item) => item.kind === 'tool')).toHaveLength(
      1,
    );
  });

  it('appends unrepresented live steps and run-level status lines in order', () => {
    const run: RunActivity = {
      runId: 'run-1',
      status: 'running',
      turns: [
        {
          turn: 1,
          finished: true,
          steps: [
            {
              kind: 'retry',
              retryNumber: 1,
              maxRetries: 3,
              reason: 'rate limited',
            },
          ],
        },
        {
          turn: 2,
          finished: false,
          steps: [toolStep(call('c2', 'shell'), { status: 'running' })],
        },
      ],
      errorMessage: null,
      historyStart: 1,
    };
    const timeline = deriveTimeline(
      [user('go'), assistant('partial')],
      null,
      [],
      run,
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'assistant',
      'status',
      'tool',
      'assistant', // pending placeholder while the run is live
    ]);
  });

  it('groups uncommitted tools recorded in the same live turn', () => {
    const run: RunActivity = {
      runId: 'run-1',
      status: 'running',
      turns: [
        {
          turn: 1,
          finished: false,
          steps: [
            toolStep(call('c1', 'read'), { status: 'running' }),
            toolStep(call('c2', 'bash'), { status: 'running' }),
          ],
        },
      ],
      errorMessage: null,
      historyStart: 1,
    };

    const timeline = deriveTimeline([user('go')], null, [], run);

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'tool-batch',
      'assistant',
    ]);
    expect(timeline.items[1]).toMatchObject({
      kind: 'tool-batch',
      tools: [
        { call: { id: 'c1' }, status: 'running' },
        { call: { id: 'c2' }, status: 'running' },
      ],
    });
  });

  it('merges live tool state into its draft batch without duplicate rows', () => {
    const read = call('c1', 'read');
    const run: RunActivity = {
      runId: 'run-1',
      status: 'running',
      turns: [
        {
          turn: 1,
          finished: false,
          steps: [
            toolStep(read, {
              status: 'done',
              content: 'file body',
            }),
          ],
        },
      ],
      errorMessage: null,
      historyStart: 1,
    };
    const timeline = deriveTimeline(
      [user('go')],
      {
        reasoning: '',
        text: '',
        tool_calls: [
          { index: 0, id: 'c1', name: 'read', arguments: '{"path":"x"}' },
          { index: 1, id: 'c2', name: 'bash', arguments: '{"command":"pwd"}' },
        ],
      },
      [],
      run,
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'tool-batch',
    ]);
    const batch = timeline.items[1];
    if (batch?.kind !== 'tool-batch') throw new Error('expected tool batch');
    expect(batch.tools).toMatchObject([
      { call: { id: 'c1' }, status: 'done', output: 'file body' },
      { call: { id: 'c2' }, status: 'running', output: null },
    ]);
  });

  it('streams the draft as an assistant row plus streaming tool rows', () => {
    const timeline = deriveTimeline([], {
      reasoning: 'inspect first',
      text: 'working on it',
      tool_calls: [{ index: 0, id: 'c1', name: 'shell', arguments: '{"cmd' }],
      fork_message_index: 4,
    });

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'assistant',
      'tool',
    ]);
    expect(timeline.items[0]).toMatchObject({
      reasoning: 'inspect first',
      streaming: true,
      messageIndex: 4,
      forkPosition: 'before_tool_calls',
    });
    expect(timeline.items[1]).toMatchObject({
      status: 'running',
      streamingArgs: '{"cmd',
    });
  });

  it('keeps a streamed tool draft unforkable until its journal is durable', () => {
    const timeline = deriveTimeline([], {
      reasoning: '',
      text: 'Preparing the tool call.',
      tool_calls: [{ index: 0, id: 'c1', name: 'shell', arguments: '{"cmd' }],
    });

    expect(timeline.items[0]).toMatchObject({
      kind: 'assistant',
      messageIndex: null,
      forkPosition: 'before_tool_calls',
    });
  });

  it('keeps committed reasoning visible when the assistant has no text', () => {
    const message = assistant('');
    message.reasoning = 'call the disk tool';

    const timeline = deriveTimeline([message], null);

    expect(timeline.items).toEqual([
      {
        kind: 'assistant',
        key: 'assistant-0',
        messageIndex: 0,
        forkPosition: 'after',
        reasoning: 'call the disk tool',
        text: '',
        streaming: false,
      },
    ]);
  });

  it('marks a visible assistant preamble as forkable before its tool batch', () => {
    const timeline = deriveTimeline(
      [
        user('inspect'),
        assistant('I will inspect with a tool.', [call('c1', 'read')]),
        toolResult('c1', 'done'),
      ],
      null,
    );

    expect(timeline.items[1]).toMatchObject({
      kind: 'assistant',
      messageIndex: 1,
      forkPosition: 'before_tool_calls',
    });
  });

  it('shows the optimistic current prompt before live activity and queues the rest', () => {
    const timeline = deriveTimeline(
      [],
      null,
      [
        {
          requestId: 'prompt-1',
          content: { type: 'text', value: 'Fix the bug' },
          status: 'accepted',
          queuePosition: null,
        },
        {
          requestId: 'prompt-2',
          content: { type: 'text', value: 'And tests' },
          status: 'queued',
          queuePosition: 1,
        },
      ],
      {
        runId: 'run-1',
        status: 'running',
        turns: [],
        errorMessage: null,
        historyStart: 0,
      },
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'assistant',
      'user',
    ]);
    expect(timeline.items[2]).toMatchObject({
      pending: { requestId: 'prompt-2' },
    });
  });

  it('keeps visible history, inserts a compaction boundary, and remaps forks', () => {
    const compaction: CompactionMarker = {
      key: 'compaction-5',
      phase: 'completed',
      historyIndex: 2,
      afterMessageCount: 2,
    };
    const timeline = deriveTimeline(
      [
        user('old request'),
        assistant('old response'),
        user('new request'),
        assistant('new response'),
      ],
      null,
      [],
      null,
      [compaction],
    );

    expect(timeline.items.map((item) => item.kind)).toEqual([
      'user',
      'assistant',
      'compaction',
      'user',
      'assistant',
    ]);
    expect(timeline.items[1]).toMatchObject({ messageIndex: null });
    expect(timeline.items[2]).toMatchObject({
      phase: 'completed',
      key: 'compaction-5',
    });
    expect(timeline.items[4]).toMatchObject({ messageIndex: 3 });
  });
});
