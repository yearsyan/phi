/** @vitest-environment jsdom */

import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionSummary } from '../types/wire.ts';
import { useSessionList } from './useSessionList.ts';

const apiMocks = vi.hoisted(() => ({
  listSessions: vi.fn(),
  setSessionPinned: vi.fn(),
  deleteSession: vi.fn(),
}));

vi.mock('../api/http.ts', () => apiMocks);

describe('useSessionList', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.listSessions.mockResolvedValue({ sessions: [], workspaces: [] });
    apiMocks.setSessionPinned.mockResolvedValue(undefined);
    apiMocks.deleteSession.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('loads once and refreshes only when explicitly requested', async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useSessionList('daemon-key', true));

    await act(async () => {
      await Promise.resolve();
    });
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(1);

    document.dispatchEvent(new Event('visibilitychange'));
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(1);

    await act(async () => {
      await result.current.refresh();
    });
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(2);
  });

  it('uses the backend workspace tree without regrouping or reordering it', async () => {
    const phiOlder = {
      ...session('019f0000-0000-7000-8000-000000000001'),
      workspace: '/workspace/phi',
    };
    const other = {
      ...session('019f0000-0000-7000-8000-000000000002'),
      workspace: '/workspace/other',
    };
    const phiNewer = {
      ...session('019f0000-0000-7000-8000-000000000003'),
      workspace: '/workspace/phi',
    };
    apiMocks.listSessions.mockResolvedValue({
      sessions: [phiNewer, other, phiOlder],
      workspaces: [
        { workspace: '/workspace/other', sessions: [other] },
        {
          workspace: '/workspace/phi',
          sessions: [phiOlder, phiNewer],
        },
      ],
    });

    const { result } = renderHook(() => useSessionList('daemon-key', true));
    await act(async () => {
      await Promise.resolve();
    });

    expect(
      result.current.workspaces.map((group) => ({
        workspace: group.workspace,
        sessions: group.sessions.map((item) => item.session_id),
      })),
    ).toEqual([
      { workspace: '/workspace/other', sessions: [other.session_id] },
      {
        workspace: '/workspace/phi',
        sessions: [phiOlder.session_id, phiNewer.session_id],
      },
    ]);
  });

  it('reloads the backend workspace tree after pinning and deletion', async () => {
    const older = {
      ...session('019f0000-0000-7000-8000-000000000001'),
      workspace: '/workspace/phi',
    };
    const newer = {
      ...session('019f0000-0000-7000-8000-000000000002'),
      workspace: '/workspace/phi',
    };
    apiMocks.listSessions
      .mockResolvedValueOnce({
        sessions: [newer, older],
        workspaces: [{ workspace: '/workspace/phi', sessions: [newer, older] }],
      })
      .mockResolvedValueOnce({
        sessions: [{ ...older, pinned: true }, newer],
        workspaces: [
          {
            workspace: '/workspace/phi',
            sessions: [{ ...older, pinned: true }, newer],
          },
        ],
      })
      .mockResolvedValueOnce({
        sessions: [newer],
        workspaces: [{ workspace: '/workspace/phi', sessions: [newer] }],
      });
    apiMocks.setSessionPinned.mockResolvedValue({ ...older, pinned: true });

    const { result } = renderHook(() => useSessionList('daemon-key', true));
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      await result.current.setPinned(older.session_id, true);
    });

    expect(apiMocks.setSessionPinned).toHaveBeenCalledWith(
      'daemon-key',
      older.session_id,
      true,
    );
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(2);
    expect(result.current.workspaces[0]?.sessions[0]?.session_id).toBe(
      older.session_id,
    );

    await act(async () => {
      await result.current.deleteSession(older.session_id);
    });
    expect(apiMocks.deleteSession).toHaveBeenCalledWith(
      'daemon-key',
      older.session_id,
    );
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(3);
    expect(
      result.current.workspaces.flatMap((group) =>
        group.sessions.map((item) => item.session_id),
      ),
    ).toEqual([newer.session_id]);
  });
});

function session(sessionId: string): SessionSummary {
  return {
    session_id: sessionId,
    title: null,
    pinned: false,
    profile_id: 'default',
    agent_profile: { agent_profile_id: 'default', revision: 0 },
    workspace: null,
    status: 'offline',
    active_run_id: null,
    queued_runs: 0,
    capability_mode: null,
    config: { model: 'test-model', reasoning_effort: null, revision: 0 },
    message_count: null,
    subagents: [],
  };
}
