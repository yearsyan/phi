import { describe, expect, it } from 'vitest';
import type {
  EventDto,
  PublicMessage,
  SessionDto,
  TokenUsage,
} from '../types/wire.ts';
import {
  initialSessionState,
  type SessionState,
  sessionReducer,
} from './sessionReducer.ts';

const zeroUsage: TokenUsage = {
  input_tokens: 0,
  output_tokens: 0,
  total_tokens: 0,
  cached_input_tokens: 0,
};

function user(text: string): PublicMessage {
  return {
    role: 'user',
    content: { type: 'text', value: text },
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function assistant(text = ''): PublicMessage {
  return {
    role: 'assistant',
    content: { type: 'text', value: text },
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  };
}

function applyEvent(
  state: SessionState,
  sequence: number,
  event: EventDto,
  runId?: string,
): SessionState {
  return sessionReducer(state, {
    type: 'event',
    envelope: { sequence, run_id: runId, event },
  });
}

function snapshot(overrides: Partial<SessionDto> = {}): SessionDto {
  return {
    session_id: 'session-1',
    profile_id: 'default',
    agent_profile: {
      agent_profile_id: 'default',
      revision: 0,
    },
    initialized: true,
    status: 'idle',
    active_run_id: null,
    queued_runs: 0,
    mode: 'default',
    capability_mode: 'full_access',
    config: {
      model: 'test-model',
      reasoning_effort: null,
      revision: 1,
    },
    history: [],
    draft: null,
    pending_asks: [],
    pending_plan_approvals: [],
    subagents: [],
    usage: {
      last: null,
      context: null,
      cumulative: zeroUsage,
    },
    last_sequence: 0,
    ...overrides,
  };
}

describe('sessionReducer', () => {
  it('tracks capability mode from ready, snapshot, and ordered events', () => {
    let state = sessionReducer(initialSessionState, {
      type: 'ready',
      config: {
        model: 'test-model',
        reasoning_effort: null,
        revision: 1,
      },
      mode: 'default',
      capabilityMode: 'read_only',
      agentProfile: {
        agent_profile_id: 'reviewer',
        revision: 3,
      },
    });
    expect(state.capabilityMode).toBe('read_only');
    expect(state.agentProfile?.agent_profile_id).toBe('reviewer');

    state = sessionReducer(state, {
      type: 'snapshot',
      session: snapshot({
        capability_mode: 'workspace_edit',
        last_sequence: 4,
      }),
    });
    state = applyEvent(state, 5, {
      type: 'capability_mode_changed',
      capability_mode: 'full_access',
    });

    expect(state.capabilityMode).toBe('full_access');
  });

  it('keeps a request ledger for multiple FIFO prompts', () => {
    let state = sessionReducer(initialSessionState, {
      type: 'local_send_prompt',
      requestId: 'prompt-1',
      content: { type: 'text', value: 'same' },
    });
    state = sessionReducer(state, {
      type: 'local_send_prompt',
      requestId: 'prompt-2',
      content: { type: 'text', value: 'same' },
    });
    state = sessionReducer(state, {
      type: 'command_accepted',
      requestId: 'prompt-2',
      queuePosition: 2,
    });
    state = sessionReducer(state, {
      type: 'command_rejected',
      requestId: 'prompt-1',
      message: 'queue_full: queue is full',
    });

    expect(state.pendingPrompts).toEqual([
      expect.objectContaining({
        requestId: 'prompt-2',
        status: 'queued',
        queuePosition: 2,
      }),
    ]);
    expect(state.notices).toEqual(['queue_full: queue is full']);
  });

  it('consumes one identical pending prompt per user message_start', () => {
    let state = initialSessionState;
    for (const requestId of ['prompt-1', 'prompt-2']) {
      state = sessionReducer(state, {
        type: 'local_send_prompt',
        requestId,
        content: { type: 'text', value: 'repeat' },
      });
    }

    state = applyEvent(state, 1, {
      type: 'message_start',
      message: user('repeat'),
    });
    state = applyEvent(state, 2, {
      type: 'message_end',
      message: user('repeat'),
    });
    expect(state.history).toHaveLength(1);
    expect(state.pendingPrompts).toHaveLength(1);

    state = applyEvent(state, 3, {
      type: 'message_start',
      message: user('repeat'),
    });
    state = applyEvent(state, 4, {
      type: 'message_end',
      message: user('repeat'),
    });
    expect(state.history).toHaveLength(2);
    expect(state.pendingPrompts).toHaveLength(0);
  });

  it('appends a new tool turn instead of overwriting the previous turn', () => {
    let state = applyEvent(initialSessionState, 1, {
      type: 'run_started',
      run_id: 'run-1',
    });
    state = applyEvent(state, 2, { type: 'turn_start', turn: 1 }, 'run-1');
    state = applyEvent(
      state,
      3,
      {
        type: 'tool_execution_start',
        call: { id: 'tool-1', name: 'read', arguments: { path: 'a.ts' } },
      },
      'run-1',
    );
    state = applyEvent(
      state,
      4,
      {
        type: 'turn_end',
        turn: 1,
        message: assistant(),
        tool_results: [],
      },
      'run-1',
    );
    state = applyEvent(state, 5, { type: 'turn_start', turn: 2 }, 'run-1');

    expect(state.activeRun?.turns.map((turn) => turn.turn)).toEqual([1, 2]);
    expect(state.activeRun?.turns[0]?.steps).toHaveLength(1);
  });

  it('applies compaction replacement and usage atomically', () => {
    const initial = sessionReducer(initialSessionState, {
      type: 'snapshot',
      session: snapshot({
        history: [user('old-1'), assistant('old-2'), user('old-3')],
        usage: {
          last: zeroUsage,
          context: {
            max_tokens: 100,
            used_tokens: 90,
            remaining_tokens: 10,
          },
          cumulative: {
            input_tokens: 10,
            output_tokens: 4,
            total_tokens: 14,
            cached_input_tokens: 0,
          },
        },
        last_sequence: 4,
      }),
    });
    const state = applyEvent(initial, 5, {
      type: 'context_compaction_completed',
      trigger: { type: 'manual', instructions: null },
      compactor: 'default',
      before_message_count: 3,
      after_message_count: 2,
      changed_from: 1,
      replacement: [user('summary')],
      summary: 'summary',
      usage: {
        input_tokens: 3,
        output_tokens: 2,
        total_tokens: 5,
        cached_input_tokens: 1,
      },
      estimated_context_tokens: 20,
    });

    expect(state.history).toEqual([user('old-1'), user('summary')]);
    expect(state.contextUsage).toBeNull();
    expect(state.usage?.context).toBeNull();
    expect(state.usage?.cumulative).toEqual({
      input_tokens: 13,
      output_tokens: 6,
      total_tokens: 19,
      cached_input_tokens: 1,
    });
  });

  it('does not let a queued run terminal event finish another active run', () => {
    const initial = sessionReducer(initialSessionState, {
      type: 'snapshot',
      session: snapshot({
        status: 'running',
        active_run_id: 'run-a',
        queued_runs: 1,
      }),
    });
    const state = applyEvent(initial, 1, {
      type: 'run_failed',
      run_id: 'run-b',
      message: 'queue cancelled',
    });

    expect(state.activeRunId).toBe('run-a');
    expect(state.activeRun?.runId).toBe('run-a');
    expect(state.status).toBe('running');
    expect(state.queuedRuns).toBe(0);
  });
});
