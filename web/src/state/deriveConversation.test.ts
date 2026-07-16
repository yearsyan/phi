import { describe, expect, it } from 'vitest';
import { deriveConversation } from './deriveConversation.ts';

describe('deriveConversation', () => {
  it('shows the optimistic current prompt before live assistant activity', () => {
    const conversation = deriveConversation(
      [],
      null,
      [
        {
          requestId: 'prompt-1',
          content: { type: 'text', value: 'Fix the bug' },
          status: 'accepted',
          queuePosition: null,
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

    expect(conversation.items.map((item) => item.kind)).toEqual([
      'user',
      'activity',
      'assistant',
    ]);
  });
});
