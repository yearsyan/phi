/** @vitest-environment jsdom */

import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useSessionList } from './useSessionList.ts';

const apiMocks = vi.hoisted(() => ({
  listSessions: vi.fn(),
}));

vi.mock('../api/http.ts', () => apiMocks);

describe('useSessionList', () => {
  beforeEach(() => {
    apiMocks.listSessions.mockReset();
    apiMocks.listSessions.mockResolvedValue({ sessions: [] });
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

    act(() => result.current.refresh());
    await act(async () => {
      await Promise.resolve();
    });
    expect(apiMocks.listSessions).toHaveBeenCalledTimes(2);
  });
});
