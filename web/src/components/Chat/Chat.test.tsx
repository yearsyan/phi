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
import type { PublicAgentProfile, PublicMessage } from '../../types/wire.ts';
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

const agentProfiles: PublicAgentProfile[] = [
  {
    agent_profile_id: 'default',
    revision: 0,
    prompt: { mode: 'full', text: '' },
    tools: { allow: null, deny: [] },
    skills: { allow: null, deny: [] },
    mcp_profile_ids: [],
    initial_capability_mode: 'workspace_edit',
    model: null,
    reasoning_effort: null,
  },
  {
    agent_profile_id: 'reviewer',
    revision: 1,
    prompt: { mode: 'extend', text: 'Review carefully.' },
    tools: { allow: null, deny: [] },
    skills: { allow: null, deny: [] },
    mcp_profile_ids: [],
    initial_capability_mode: 'read_only',
    model: null,
    reasoning_effort: null,
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
    canReconfigurePreparedSession: true,
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
          agentProfileId="default"
          agentProfiles={agentProfiles}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectAgentProfile={vi.fn()}
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

  it('selects an Agent Profile before the first prompt', () => {
    const onSelectAgentProfile = vi.fn();

    render(
      <I18nProvider initialLocale="en">
        <Chat
          controls={controls()}
          authKey="daemon-key"
          profileId="default"
          providerProfiles={[]}
          agentProfileId="default"
          agentProfiles={agentProfiles}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectAgentProfile={onSelectAgentProfile}
          onSelectWorkspace={vi.fn()}
          onOpenSidebar={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </I18nProvider>,
    );

    const picker = screen.getByRole('combobox', { name: 'Agent Profile' });
    expect((picker as HTMLSelectElement).disabled).toBe(false);
    expect((picker as HTMLSelectElement).value).toBe('default');

    fireEvent.change(picker, { target: { value: 'reviewer' } });
    expect(onSelectAgentProfile).toHaveBeenCalledWith('reviewer');
  });

  it('shows the pinned Agent Profile but locks it after activation', () => {
    const onSelectAgentProfile = vi.fn();
    const session = controls({
      sessionId: 'session-1',
      history,
      agentProfile: { agent_profile_id: 'reviewer', revision: 1 },
    });

    render(
      <I18nProvider initialLocale="en">
        <Chat
          controls={session}
          authKey="daemon-key"
          profileId="default"
          providerProfiles={[]}
          agentProfileId="default"
          agentProfiles={agentProfiles}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectAgentProfile={onSelectAgentProfile}
          onSelectWorkspace={vi.fn()}
          onOpenSidebar={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </I18nProvider>,
    );

    const picker = screen.getByRole('combobox', { name: 'Agent Profile' });
    expect((picker as HTMLSelectElement).value).toBe('reviewer');
    expect((picker as HTMLSelectElement).disabled).toBe(true);

    fireEvent.change(picker, { target: { value: 'default' } });
    expect(onSelectAgentProfile).not.toHaveBeenCalled();
  });

  it('locks Agent Profile selection as soon as the first prompt is admitted', () => {
    const session = controls();
    session.canReconfigurePreparedSession = false;

    render(
      <I18nProvider initialLocale="en">
        <Chat
          controls={session}
          authKey="daemon-key"
          profileId="default"
          providerProfiles={[]}
          agentProfileId="default"
          agentProfiles={agentProfiles}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectAgentProfile={vi.fn()}
          onSelectWorkspace={vi.fn()}
          onOpenSidebar={vi.fn()}
          onOpenSettings={vi.fn()}
        />
      </I18nProvider>,
    );

    const picker = screen.getByRole('combobox', { name: 'Agent Profile' });
    expect((picker as HTMLSelectElement).disabled).toBe(true);
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
          agentProfileId="default"
          agentProfiles={agentProfiles}
          onFork={vi.fn()}
          onSelectProvider={vi.fn()}
          onSelectAgentProfile={vi.fn()}
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
            agentProfileId="default"
            agentProfiles={agentProfiles}
            onFork={vi.fn()}
            onSelectProvider={vi.fn()}
            onSelectAgentProfile={vi.fn()}
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
