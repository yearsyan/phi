/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type {
  SessionSummary,
  WorkspaceSessionGroup,
} from '../../types/wire.ts';
import { Sidebar } from './Sidebar.tsx';

const activatedSession: SessionSummary = {
  session_id: 'abc123',
  title: null,
  pinned: false,
  profile_id: 'default',
  agent_profile: {
    agent_profile_id: 'default',
    revision: 0,
  },
  workspace: '/workspace/phi',
  status: 'idle',
  active_run_id: null,
  queued_runs: 0,
  capability_mode: 'full_access',
  config: {
    model: 'test-model',
    reasoning_effort: null,
    revision: 1,
  },
  message_count: 2,
  subagents: [],
};

function workspaceGroup(
  workspace: string | null,
  sessions: SessionSummary[],
): WorkspaceSessionGroup {
  return { workspace, sessions };
}

function renderSidebar(workspaces: WorkspaceSessionGroup[]) {
  return render(
    <I18nProvider initialLocale="en">
      <Sidebar
        open
        workspaces={workspaces}
        loading={false}
        activeSessionId={null}
        listError={null}
        profileId="default"
        theme="light"
        onSelect={vi.fn()}
        onSetPinned={vi.fn()}
        onDelete={vi.fn()}
        onNewChat={vi.fn()}
        onOpenSettings={vi.fn()}
        onToggleTheme={vi.fn()}
        onCycleLocale={vi.fn()}
        onClose={vi.fn()}
      />
    </I18nProvider>,
  );
}

describe('Sidebar', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows only activated sessions in the recent list', () => {
    const view = renderSidebar([]);

    expect(screen.queryByText('Phi')).toBeNull();
    expect(screen.queryByText('coding workspace')).toBeNull();
    expect(screen.getByText('No sessions yet.')).toBeTruthy();
    expect(screen.queryByText('New session')).toBeNull();

    view.rerender(
      <I18nProvider initialLocale="en">
        <Sidebar
          open
          workspaces={[workspaceGroup('/workspace/phi', [activatedSession])]}
          loading={false}
          activeSessionId="abc123"
          listError={null}
          profileId="default"
          theme="light"
          onSelect={vi.fn()}
          onSetPinned={vi.fn()}
          onDelete={vi.fn()}
          onNewChat={vi.fn()}
          onOpenSettings={vi.fn()}
          onToggleTheme={vi.fn()}
          onCycleLocale={vi.fn()}
          onClose={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(screen.queryByText('No sessions yet.')).toBeNull();
    expect(screen.getByText('Session abc123')).toBeTruthy();
  });

  it('shows an automatic session title when one is available', () => {
    renderSidebar([
      workspaceGroup('/workspace/phi', [
        {
          ...activatedSession,
          title: 'Fix flaky storage tests',
        },
      ]),
    ]);

    expect(screen.getByText('Fix flaky storage tests')).toBeTruthy();
    expect(screen.queryByText('Session abc123')).toBeNull();
  });

  it('renders the backend workspace tree without regrouping its sessions', () => {
    renderSidebar([
      workspaceGroup('/workspace/phi', [
        {
          ...activatedSession,
          session_id: 'phi-new',
          title: 'Newest Phi task',
        },
        {
          ...activatedSession,
          session_id: 'phi-old',
          title: 'Older Phi task',
        },
      ]),
      workspaceGroup('/workspace/other', [
        {
          ...activatedSession,
          session_id: 'other-task',
          title: 'Other workspace task',
          workspace: '/workspace/other',
        },
      ]),
      workspaceGroup(null, [
        {
          ...activatedSession,
          session_id: 'legacy-task',
          title: 'Legacy task',
          workspace: null,
        },
      ]),
    ]);

    const workspaceNodes = screen.getAllByRole('button', {
      name: /, \d+ sessions$/,
    });
    expect(
      workspaceNodes.map((node) => node.getAttribute('aria-label')),
    ).toEqual(['phi, 2 sessions', 'other, 1 sessions', 'Other, 1 sessions']);

    const phiSessions = screen.getByRole('group', { name: 'phi' });
    expect(
      within(phiSessions)
        .getAllByRole('button')
        .map((node) => node.textContent),
    ).toEqual([
      expect.stringContaining('Newest Phi task'),
      expect.stringContaining('Older Phi task'),
    ]);
  });

  it('collapses and expands a workspace branch', () => {
    renderSidebar([
      workspaceGroup('/workspace/phi', [
        { ...activatedSession, session_id: 'phi-one', title: 'First task' },
        { ...activatedSession, session_id: 'phi-two', title: 'Second task' },
      ]),
    ]);

    const workspaceNode = screen.getByRole('button', {
      name: 'phi, 2 sessions',
    });
    expect(workspaceNode.getAttribute('aria-expanded')).toBe('true');

    fireEvent.click(workspaceNode);
    expect(workspaceNode.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('First task')).toBeNull();
    expect(screen.queryByRole('group', { name: 'phi' })).toBeNull();

    fireEvent.click(workspaceNode);
    expect(workspaceNode.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('First task')).toBeTruthy();
  });

  it('opens session actions on right click and toggles pinning', async () => {
    const onSetPinned = vi.fn().mockResolvedValue(undefined);
    const onDelete = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <Sidebar
          open
          workspaces={[workspaceGroup('/workspace/phi', [activatedSession])]}
          loading={false}
          activeSessionId={null}
          listError={null}
          profileId="default"
          theme="light"
          onSelect={vi.fn()}
          onSetPinned={onSetPinned}
          onDelete={onDelete}
          onNewChat={vi.fn()}
          onOpenSettings={vi.fn()}
          onToggleTheme={vi.fn()}
          onCycleLocale={vi.fn()}
          onClose={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.contextMenu(screen.getByText('Session abc123'));
    expect(screen.getByRole('menu', { name: 'Session actions' })).toBeTruthy();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Pin' }));

    await waitFor(() =>
      expect(onSetPinned).toHaveBeenCalledWith('abc123', true),
    );
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('confirms deletion from the right-click menu', async () => {
    const onDelete = vi.fn().mockResolvedValue(undefined);
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(
      <I18nProvider initialLocale="en">
        <Sidebar
          open
          workspaces={[
            workspaceGroup('/workspace/phi', [
              { ...activatedSession, title: 'Delete me' },
            ]),
          ]}
          loading={false}
          activeSessionId={null}
          listError={null}
          profileId="default"
          theme="light"
          onSelect={vi.fn()}
          onSetPinned={vi.fn()}
          onDelete={onDelete}
          onNewChat={vi.fn()}
          onOpenSettings={vi.fn()}
          onToggleTheme={vi.fn()}
          onCycleLocale={vi.fn()}
          onClose={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.contextMenu(screen.getByText('Delete me'));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete' }));

    expect(confirm).toHaveBeenCalledWith(
      'Delete “Delete me”? This cannot be undone.',
    );
    await waitFor(() => expect(onDelete).toHaveBeenCalledWith('abc123'));
  });
});
