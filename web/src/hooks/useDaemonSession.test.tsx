/** @vitest-environment jsdom */

import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  SessionSocket,
  SessionSocketHandlers,
  SessionSocketOpenOptions,
} from '../ws/connection.ts';
import {
  SESSION_CONNECT_TIMEOUT_MS,
  useDaemonSession,
} from './useDaemonSession.ts';

const sessionConnectionMocks = vi.hoisted(() => ({
  openNewSession: vi.fn(),
  attachSession: vi.fn(),
}));

vi.mock('../ws/sessionConnection.ts', () => sessionConnectionMocks);

const READY_MESSAGE = {
  type: 'ready',
  config: {
    model: 'test-model',
    reasoning_effort: null,
    revision: 1,
  },
  mode: 'default',
} as const;

const NEW_TARGET = {
  kind: 'new',
  profileId: 'default',
} as const;

function fakeSocket() {
  return {
    isOpen: true,
    send: vi.fn(),
    close: vi.fn(),
  };
}

describe('useDaemonSession', () => {
  beforeEach(() => {
    sessionConnectionMocks.openNewSession.mockReset();
    sessionConnectionMocks.attachSession.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('moves to error on disconnect and does not create phantom prompts', async () => {
    const socket = fakeSocket();
    let handlers: SessionSocketHandlers | null = null;
    sessionConnectionMocks.openNewSession.mockImplementation(
      async (
        _authKey: string,
        _profileId: string,
        nextHandlers: SessionSocketHandlers,
      ): Promise<SessionSocket> => {
        handlers = nextHandlers;
        return socket as unknown as SessionSocket;
      },
    );

    const { result } = renderHook(() =>
      useDaemonSession('daemon-key', NEW_TARGET),
    );

    await waitFor(() => {
      expect(result.current.connectionPhase).toBe('preparing');
    });

    act(() => {
      handlers?.onMessage(READY_MESSAGE);
    });
    expect(result.current.connectionPhase).toBe('ready');
    expect(result.current.state.ready).toBe(true);

    act(() => {
      result.current.sendPrompt('delivered');
    });
    expect(socket.send).toHaveBeenCalledTimes(1);
    expect(result.current.state.pendingUser).not.toBeNull();

    act(() => {
      handlers?.onClose(
        new CloseEvent('close', { code: 1006, reason: 'connection lost' }),
      );
    });
    expect(result.current.connectionPhase).toBe('error');
    expect(result.current.state.ready).toBe(false);
    expect(result.current.state.pendingUser).toBeNull();

    act(() => {
      result.current.sendPrompt('must not be displayed');
    });
    expect(socket.send).toHaveBeenCalledTimes(1);
    expect(result.current.state.pendingUser).toBeNull();
  });

  it('times out a connection that never becomes ready', async () => {
    vi.useFakeTimers();
    sessionConnectionMocks.openNewSession.mockImplementation(
      (
        _authKey: string,
        _profileId: string,
        _handlers: SessionSocketHandlers,
        options?: SessionSocketOpenOptions,
      ) =>
        new Promise<SessionSocket>((_resolve, reject) => {
          options?.signal?.addEventListener(
            'abort',
            () => reject(options.signal?.reason),
            { once: true },
          );
        }),
    );

    const { result } = renderHook(() =>
      useDaemonSession('daemon-key', NEW_TARGET),
    );

    await act(async () => {
      await vi.advanceTimersByTimeAsync(SESSION_CONNECT_TIMEOUT_MS);
    });

    expect(result.current.connectionPhase).toBe('error');
    expect(result.current.connectionError).toContain('timed out');
    expect(result.current.state.ready).toBe(false);
  });

  it('retries the current target without requiring a selection change', async () => {
    const socket = fakeSocket();
    sessionConnectionMocks.openNewSession
      .mockRejectedValueOnce(new Error('daemon unavailable'))
      .mockResolvedValueOnce(socket as unknown as SessionSocket);

    const { result } = renderHook(() =>
      useDaemonSession('daemon-key', NEW_TARGET),
    );

    await waitFor(() => {
      expect(result.current.connectionPhase).toBe('error');
    });

    act(() => {
      result.current.retry();
    });

    await waitFor(() => {
      expect(sessionConnectionMocks.openNewSession).toHaveBeenCalledTimes(2);
      expect(result.current.connectionPhase).toBe('preparing');
    });
  });
});
