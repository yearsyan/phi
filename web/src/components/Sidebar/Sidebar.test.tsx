/** @vitest-environment jsdom */

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { SessionSummary } from '../../types/wire.ts';
import { Sidebar } from './Sidebar.tsx';

const activatedSession: SessionSummary = {
  session_id: 'abc123',
  profile_id: 'default',
  agent_profile: {
    agent_profile_id: 'default',
    revision: 0,
  },
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
  message_count: 2,
  subagents: [],
};

function renderSidebar(sessions: SessionSummary[]) {
  return render(
    <I18nProvider initialLocale="en">
      <Sidebar
        open
        sessions={sessions}
        loading={false}
        activeSessionId={null}
        listError={null}
        profileId="default"
        theme="light"
        onSelect={vi.fn()}
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
  afterEach(cleanup);

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
          sessions={[activatedSession]}
          loading={false}
          activeSessionId="abc123"
          listError={null}
          profileId="default"
          theme="light"
          onSelect={vi.fn()}
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
});
