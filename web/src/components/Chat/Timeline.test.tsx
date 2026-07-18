/** @vitest-environment jsdom */

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { TimelineItem } from '../../state/timeline.ts';
import { TimelineRow, withTimelineTailSpacer } from './Timeline.tsx';

function renderRow(item: TimelineItem) {
  return render(
    <I18nProvider initialLocale="en">
      <TimelineRow item={item} />
    </I18nProvider>,
  );
}

describe('TimelineRow', () => {
  afterEach(cleanup);

  it('dispatches a user item', () => {
    renderRow({
      kind: 'user',
      key: 'user-0',
      message: {
        role: 'user',
        content: { type: 'text', value: 'Fix the bug' },
        tool_calls: [],
        tool_call_id: null,
        tool_result_is_error: false,
      },
      pending: null,
    });
    expect(screen.getByText('Fix the bug')).toBeTruthy();
  });

  it('dispatches an assistant item', () => {
    renderRow({
      kind: 'assistant',
      key: 'assistant-1',
      messageIndex: 1,
      forkPosition: 'after',
      reasoning: '',
      text: 'all done',
      streaming: false,
    });
    expect(screen.getByText('all done')).toBeTruthy();
  });

  it('dispatches a tool item', () => {
    renderRow({
      kind: 'tool',
      key: 'tool-c1',
      call: { id: 'c1', name: 'read', arguments: { path: '/tmp/a' } },
      status: 'done',
      progress: [],
      output: 'file body',
      streamingArgs: null,
    });
    expect(screen.getByText('read')).toBeTruthy();
  });

  it('dispatches a multi-tool batch as one collapsed summary', () => {
    renderRow({
      kind: 'tool-batch',
      key: 'tool-batch:c1',
      tools: [
        {
          kind: 'tool',
          key: 'tool-c1',
          call: { id: 'c1', name: 'read', arguments: { path: '/tmp/a' } },
          status: 'done',
          progress: [],
          output: 'file body',
          streamingArgs: null,
        },
        {
          kind: 'tool',
          key: 'tool-c2',
          call: { id: 'c2', name: 'bash', arguments: { command: 'pwd' } },
          status: 'done',
          progress: [],
          output: '/tmp',
          streamingArgs: null,
        },
      ],
    });

    expect(
      screen.getByRole('button', { name: /Reading and Executing/ }),
    ).toBeTruthy();
    expect(screen.queryByText('read')).toBeNull();
  });

  it('dispatches a status item', () => {
    renderRow({
      kind: 'status',
      key: 'status-0',
      step: { kind: 'notice', level: 'warn', message: 'heads up' },
    });
    expect(screen.getByText('heads up')).toBeTruthy();
  });

  it('renders compaction progress as a divider and then as a boundary', () => {
    const { rerender } = renderRow({
      kind: 'compaction',
      key: 'compaction-1',
      phase: 'started',
    });
    expect(screen.getByText('Compacting context…')).toBeTruthy();

    rerender(
      <I18nProvider initialLocale="en">
        <TimelineRow
          item={{
            kind: 'compaction',
            key: 'compaction-1',
            phase: 'completed',
          }}
        />
      </I18nProvider>,
    );
    expect(screen.getByText('Context compacted')).toBeTruthy();
  });
});

describe('Timeline', () => {
  it('appends the composer safety space after the latest message', () => {
    const assistant: TimelineItem = {
      kind: 'assistant',
      key: 'assistant-1',
      messageIndex: null,
      forkPosition: 'after',
      reasoning: '',
      text: 'streaming answer',
      streaming: true,
    };

    const virtualItems = withTimelineTailSpacer([assistant]);

    expect(virtualItems[0]).toBe(assistant);
    expect(virtualItems.at(-1)).toEqual({
      kind: 'tail-spacer',
      key: 'timeline-tail-spacer',
    });
  });
});
