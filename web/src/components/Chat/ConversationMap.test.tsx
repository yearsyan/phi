/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { TimelineItem } from '../../state/timeline.ts';
import {
  activeConversationMapEntryKey,
  buildConversationMapEntries,
  ConversationMap,
} from './ConversationMap.tsx';

const items: TimelineItem[] = [
  {
    kind: 'user',
    key: 'user-0',
    message: {
      role: 'user',
      content: { type: 'text', value: 'Fix **the sidebar** spacing' },
      tool_calls: [],
      tool_call_id: null,
      tool_result_is_error: false,
    },
    pending: null,
  },
  {
    kind: 'tool-batch',
    key: 'tool-batch:one',
    tools: [
      {
        kind: 'tool',
        key: 'tool-one',
        call: {
          id: 'one',
          name: 'read',
          arguments: { path: '/workspace/Sidebar.module.css' },
        },
        status: 'done',
        progress: [],
        output: 'body',
        streamingArgs: null,
      },
      {
        kind: 'tool',
        key: 'tool-two',
        call: { id: 'two', name: 'bash', arguments: { command: 'pnpm test' } },
        status: 'done',
        progress: [],
        output: 'ok',
        streamingArgs: null,
      },
    ],
  },
  {
    kind: 'assistant',
    key: 'assistant-2',
    messageIndex: 2,
    forkPosition: 'after',
    reasoning: '',
    text: 'The scrollbar now reaches the sidebar edge.',
    streaming: false,
  },
  {
    kind: 'user',
    key: 'user-3',
    message: {
      role: 'user',
      content: { type: 'text', value: 'Add a conversation preview map' },
      tool_calls: [],
      tool_call_id: null,
      tool_result_is_error: false,
    },
    pending: null,
  },
  {
    kind: 'assistant',
    key: 'assistant-4',
    messageIndex: 4,
    forkPosition: 'after',
    reasoning: '',
    text: 'Added hover previews and direct navigation.',
    streaming: false,
  },
];

afterEach(cleanup);

describe('conversation map projection', () => {
  it('groups tool-heavy activity into its owning user turn', () => {
    expect(buildConversationMapEntries(items)).toEqual([
      {
        key: 'map-user-0',
        startIndex: 0,
        endIndex: 2,
        kind: 'turn',
        title: 'Fix the sidebar spacing',
        excerpt: 'The scrollbar now reaches the sidebar edge.',
        resources: ['Sidebar.module.css', 'bash'],
      },
      {
        key: 'map-user-3',
        startIndex: 3,
        endIndex: 4,
        kind: 'turn',
        title: 'Add a conversation preview map',
        excerpt: 'Added hover previews and direct navigation.',
        resources: [],
      },
    ]);
  });

  it('selects the turn nearest the center of the visible range', () => {
    const entries = buildConversationMapEntries(items);
    expect(
      activeConversationMapEntryKey(entries, { startIndex: 0, endIndex: 2 }),
    ).toBe('map-user-0');
    expect(
      activeConversationMapEntryKey(entries, { startIndex: 3, endIndex: 5 }),
    ).toBe('map-user-3');
    expect(activeConversationMapEntryKey(entries, null)).toBe('map-user-3');
  });
});

describe('ConversationMap', () => {
  it('shows turn previews and jumps to the selected timeline index', () => {
    const onJump = vi.fn();
    render(
      <I18nProvider initialLocale="en">
        <ConversationMap
          items={items}
          visibleRange={{ startIndex: 0, endIndex: 2 }}
          onJump={onJump}
        />
      </I18nProvider>,
    );

    const first = screen.getByRole('button', {
      name: 'Jump to: Fix the sidebar spacing',
    });
    expect(
      screen
        .getByRole('navigation', { name: 'Conversation outline' })
        .style.getPropertyValue('--map-track-height'),
    ).toBe('32px');
    expect(first.getAttribute('aria-current')).toBe('location');
    expect(screen.getByText('Sidebar.module.css')).toBeTruthy();
    expect(
      screen.getByText('The scrollbar now reaches the sidebar edge.'),
    ).toBeTruthy();

    fireEvent.click(
      screen.getByRole('button', {
        name: 'Jump to: Add a conversation preview map',
      }),
    );
    expect(onJump).toHaveBeenCalledWith(3);
  });
});
