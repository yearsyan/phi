/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../i18n/I18nProvider.tsx';
import { AssistantText } from './AssistantText.tsx';
import { StatusLine } from './StatusLine.tsx';
import { UserMessage } from './UserMessage.tsx';

function withI18n(node: React.ReactElement) {
  return render(<I18nProvider initialLocale="en">{node}</I18nProvider>);
}

describe('UserMessage', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('renders the prompt text and the queued position while pending', () => {
    withI18n(
      <UserMessage
        message={{
          role: 'user',
          content: { type: 'text', value: 'Fix the bug' },
          tool_calls: [],
          tool_call_id: null,
          tool_result_is_error: false,
        }}
        pending={{
          requestId: 'p1',
          content: { type: 'text', value: 'Fix the bug' },
          status: 'queued',
          queuePosition: 2,
        }}
      />,
    );

    expect(screen.getByText('Fix the bug')).toBeTruthy();
    expect(screen.getByText('Queued at position 2')).toBeTruthy();
  });

  it('copies the complete visible user message and reports success', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    withI18n(
      <UserMessage
        message={{
          role: 'user',
          content: {
            type: 'parts',
            value: [
              { type: 'text', text: 'Review ' },
              {
                type: 'document',
                document: {
                  filename: 'spec.pdf',
                  mime_type: 'application/pdf',
                  data: 'data:application/pdf;base64,AA==',
                },
              },
            ],
          },
          tool_calls: [],
          tool_call_id: null,
          tool_result_is_error: false,
        }}
        pending={null}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith('Review [spec.pdf]'),
    );
    expect(screen.getByRole('button', { name: 'Copied' })).toBeTruthy();
  });

  it('reports a user-message clipboard failure', async () => {
    vi.stubGlobal('navigator', {});
    withI18n(
      <UserMessage
        message={{
          role: 'user',
          content: { type: 'text', value: 'Cannot copy this' },
          tool_calls: [],
          tool_call_id: null,
          tool_result_is_error: false,
        }}
        pending={null}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));

    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toBe('Copy failed'),
    );
  });
});

describe('AssistantText', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('renders markdown text', () => {
    withI18n(<AssistantText text="**bold** answer" streaming={false} />);
    expect(screen.getByText('bold').tagName).toBe('STRONG');
  });

  it('shows the thinking placeholder for an empty streaming row', () => {
    withI18n(<AssistantText text="" streaming={true} />);
    expect(screen.getByText('Thinking…')).toBeTruthy();
  });

  it('renders completed reasoning in a collapsed Kimi-style disclosure', () => {
    withI18n(
      <AssistantText
        reasoning="Inspect the inputs first."
        text="Done."
        streaming={false}
      />,
    );

    const label = screen.getByText('Thought');
    const details = label.closest('details');
    expect(details).toBeTruthy();
    expect(details?.open).toBe(false);
    expect(screen.getByText('Inspect the inputs first.')).toBeTruthy();
    expect(screen.getByText('Done.')).toBeTruthy();
  });

  it('uses the reasoning trigger instead of a duplicate streaming placeholder', () => {
    withI18n(
      <AssistantText reasoning="Inspecting inputs" text="" streaming={true} />,
    );

    expect(screen.getAllByText('Thinking…')).toHaveLength(1);
  });

  it('copies the visible response and forks from its transcript index', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const onFork = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    withI18n(
      <AssistantText
        messageIndex={7}
        text="Copy this answer"
        streaming={false}
        forkEnabled
        onFork={onFork}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy response' }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith('Copy this answer'),
    );
    expect(screen.getByRole('button', { name: 'Copied' })).toBeTruthy();

    fireEvent.click(
      screen.getByRole('button', { name: 'Fork from this response' }),
    );
    await waitFor(() => expect(onFork).toHaveBeenCalledWith(7, 'after'));
  });

  it('labels and submits the tool-call preamble fork position', async () => {
    const onFork = vi.fn().mockResolvedValue(undefined);
    withI18n(
      <AssistantText
        messageIndex={9}
        forkPosition="before_tool_calls"
        text="I will inspect with a tool."
        streaming
        forkEnabled
        onFork={onFork}
      />,
    );

    fireEvent.click(
      screen.getByRole('button', { name: 'Fork before these tool calls' }),
    );
    await waitFor(() =>
      expect(onFork).toHaveBeenCalledWith(9, 'before_tool_calls'),
    );
  });
});

describe('StatusLine', () => {
  afterEach(cleanup);

  it('formats retry steps with counts and reason', () => {
    withI18n(
      <StatusLine
        step={{
          kind: 'retry',
          retryNumber: 2,
          maxRetries: 5,
          reason: 'rate limited',
        }}
      />,
    );
    expect(screen.getByText('Retry 2/5 · rate limited')).toBeTruthy();
  });

  it('passes notice messages through with their level', () => {
    withI18n(
      <StatusLine
        step={{ kind: 'notice', level: 'error', message: 'run failed' }}
      />,
    );
    expect(screen.getByText('run failed')).toBeTruthy();
  });
});
