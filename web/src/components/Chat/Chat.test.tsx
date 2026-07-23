/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DaemonSessionControls } from '../../hooks/useDaemonSession.ts';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import { initialSessionState } from '../../state/sessionReducer.ts';
import type { PublicMessage } from '../../types/wire.ts';
import { Chat } from './Chat.tsx';
import { workspaceName } from './WorkspacePicker.tsx';

const history: PublicMessage[] = [
  {
    role: 'user',
    content: { type: 'text', value: 'Choose an option' },
    tool_calls: [],
    tool_call_id: null,
    tool_result_is_error: false,
  },
];

function controls(
  overrides: Partial<DaemonSessionControls['state']> = {},
): DaemonSessionControls {
  return {
    state: {
      ...initialSessionState,
      title: 'Optimize session header',
      workspace: '/Users/u/workspace/phi',
      ready: true,
      status: 'idle',
      config: {
        model: 'test-model',
        reasoning_effort: null,
        revision: 1,
      },
      ...overrides,
    },
    connectionPhase: 'ready',
    connectionError: null,
    sessionListRevision: 0,
    retry: vi.fn(),
    sendPrompt: vi.fn(() => true),
    stop: vi.fn(),
    answerAsk: vi.fn(() => true),
    decideToolPermission: vi.fn(() => true),
    setModel: vi.fn(),
    setReasoningEffort: vi.fn(),
    setCapabilityMode: vi.fn(),
    compact: vi.fn(() => true),
    clearNotice: vi.fn(),
  };
}

describe('Chat', () => {
  afterEach(cleanup);

  it('shows the title, compact workspace name, and right-side menu', () => {
    const session = controls();

    render(
      <I18nProvider initialLocale="en">
        <Chat
          controls={session}
          authKey="daemon-key"
          profileId="default"
          providerProfiles={[]}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectWorkspace={vi.fn()}
          onOpenSidebar={vi.fn()}
          onOpenSettings={session.retry}
        />
      </I18nProvider>,
    );

    expect(screen.getByText('Optimize session header')).toBeTruthy();
    const workspace = within(screen.getByRole('banner')).getByText(
      'phi',
    ).parentElement;
    expect(workspace).toBeTruthy();
    expect(workspace?.getAttribute('title')).toBe('/Users/u/workspace/phi');
    expect(
      screen.getByText('Working directory: /Users/u/workspace/phi'),
    ).toBeTruthy();
    expect(screen.queryByText('idle')).toBeNull();
    expect(screen.queryByRole('heading', { level: 1 })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect(session.retry).toHaveBeenCalledOnce();
  });

  it('derives a readable directory name without losing root paths', () => {
    expect(workspaceName('/Users/u/workspace/phi/')).toBe('phi');
    expect(workspaceName('C:\\work\\phi\\')).toBe('phi');
    expect(workspaceName('/')).toBe('/');
  });

  it('anchors pending questions and the composer in the same interaction dock', () => {
    const session = controls({
      sessionId: 'session-1',
      history,
      pendingAsks: [
        {
          ask_id: 'ask-1',
          questions: [
            {
              header: 'Scope',
              question: 'What should change?',
              multiSelect: false,
              options: [
                { label: 'Layout', description: 'Fix the floating panel' },
              ],
            },
          ],
        },
      ],
    });

    render(
      <I18nProvider initialLocale="en">
        <Chat
          controls={session}
          authKey="daemon-key"
          profileId="default"
          providerProfiles={[]}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectWorkspace={vi.fn()}
          onOpenSidebar={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </I18nProvider>,
    );

    const dialog = screen.getByRole('dialog', {
      name: 'The assistant needs your input',
    });
    const panels = dialog.parentElement;
    const interactionDock = panels?.parentElement;
    const composer = screen.getByLabelText('Message Phi').closest('footer');

    expect(interactionDock).toBeTruthy();
    expect(composer).toBeTruthy();
    expect(interactionDock?.firstElementChild).toBe(panels);
    expect(interactionDock?.lastElementChild).toBe(composer);
  });

  it('reserves the measured floating composer height in the scroll timeline', () => {
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockReturnValue({
        bottom: 144,
        height: 144,
        left: 0,
        right: 900,
        top: 0,
        width: 900,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      });

    try {
      const session = controls({ sessionId: 'session-1', history });
      render(
        <I18nProvider initialLocale="en">
          <Chat
            controls={session}
            authKey="daemon-key"
            profileId="default"
            providerProfiles={[]}
            onFork={vi.fn()}
            onSelectProvider={vi.fn()}
            onSelectWorkspace={vi.fn()}
            onOpenSidebar={vi.fn()}
            onOpenSettings={vi.fn()}
          />
        </I18nProvider>,
      );

      const chat = screen.getByLabelText('Message Phi').closest('section');
      expect(chat?.style.getPropertyValue('--interaction-dock-height')).toBe(
        '144px',
      );
    } finally {
      rectSpy.mockRestore();
    }
  });
});
