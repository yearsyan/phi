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
  SESSION_RECONNECT_DELAYS_MS,
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
  capability_mode: 'full_access',
  agent_profile: {
    agent_profile_id: 'default',
    revision: 0,
  },
  skills: [
    {
      name: 'review',
      description: 'Review the current change',
      model_invocable: true,
      user_invocable: true,
    },
  ],
} as const;

const NEW_TARGET = {
  kind: 'new',
  profileId: 'default',
  instanceId: 1,
} as const;

const RESTRICTED_PROFILE_TARGET = {
  ...NEW_TARGET,
  agentProfileId: 'reviewer',
  capabilityMode: 'read_only',
  workspace: '/workspace/review',
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

  it('keeps the activated prepared session on its original socket', async () => {
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
      handlers?.onMessage({
        type: 'session_created',
        session_id: 'activated-session',
      });
    });

    expect(result.current.connectionPhase).toBe('ready');
    expect(result.current.state.sessionId).toBe('activated-session');
    expect(result.current.sessionListRevision).toBe(1);

    act(() => {
      handlers?.onMessage({
        type: 'event',
        sequence: 1,
        session_id: 'activated-session',
        event: {
          type: 'title_changed',
          title: 'Generated title',
        },
      });
    });

    expect(result.current.state.title).toBe('Generated title');
    expect(result.current.sessionListRevision).toBe(2);
    expect(socket.close).not.toHaveBeenCalled();
    expect(sessionConnectionMocks.attachSession).not.toHaveBeenCalled();
  });

  it('passes Agent Profile and capability defaults to a prepared session', async () => {
    const socket = fakeSocket();
    sessionConnectionMocks.openNewSession.mockResolvedValue(
      socket as unknown as SessionSocket,
    );

    renderHook(() => useDaemonSession('daemon-key', RESTRICTED_PROFILE_TARGET));

    await waitFor(() => {
      expect(sessionConnectionMocks.openNewSession).toHaveBeenCalledWith(
        'daemon-key',
        'default',
        expect.any(Object),
        expect.objectContaining({
          agentProfileId: 'reviewer',
          capabilityMode: 'read_only',
          workspace: '/workspace/review',
        }),
      );
    });
  });

  it('tracks multiple optimistic prompts and clears them on disconnect', async () => {
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
    act(() => handlers?.onMessage(READY_MESSAGE));

    act(() => {
      expect(result.current.sendPrompt('first')).toBe(true);
      expect(result.current.sendPrompt('second')).toBe(true);
    });
    expect(socket.send).toHaveBeenCalledTimes(2);
    expect(result.current.state.pendingPrompts).toHaveLength(2);

    act(() => {
      handlers?.onClose(
        new CloseEvent('close', { code: 1006, reason: 'connection lost' }),
      );
    });
    // A first prompt without session_created is an ambiguous delivery window:
    // never resend it automatically.
    expect(result.current.connectionPhase).toBe('error');
    expect(result.current.state.pendingPrompts).toHaveLength(0);

    act(() => {
      expect(result.current.sendPrompt('must not be displayed')).toBe(false);
    });
    expect(socket.send).toHaveBeenCalledTimes(2);
    expect(result.current.state.pendingPrompts).toHaveLength(0);
  });

  it('exposes ready skills and sends explicit skill and reasoning commands', async () => {
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
    act(() => handlers?.onMessage(READY_MESSAGE));

    expect(result.current.state.skills[0]?.name).toBe('review');
    act(() => {
      expect(result.current.sendPrompt('security', { name: 'review' })).toBe(
        true,
      );
      result.current.setReasoningEffort('high');
    });

    expect(socket.send).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        type: 'prompt',
        content: { type: 'text', value: 'security' },
        skill: expect.objectContaining({ name: 'review' }),
      }),
    );
    expect(socket.send).toHaveBeenNthCalledWith(2, {
      type: 'set_reasoning_effort',
      request_id: expect.any(String),
      effort: 'high',
    });
    expect(result.current.state.pendingPrompts[0]).toEqual(
      expect.objectContaining({
        content: { type: 'text', value: '/review security' },
        matchAnyEcho: true,
      }),
    );
  });

  it('reattaches after an activated session disconnects', async () => {
    vi.useFakeTimers();
    const first = fakeSocket();
    const second = fakeSocket();
    let handlers: SessionSocketHandlers | null = null;
    sessionConnectionMocks.openNewSession.mockImplementation(
      async (
        _authKey: string,
        _profileId: string,
        nextHandlers: SessionSocketHandlers,
      ): Promise<SessionSocket> => {
        handlers = nextHandlers;
        return first as unknown as SessionSocket;
      },
    );
    sessionConnectionMocks.attachSession.mockResolvedValue(
      second as unknown as SessionSocket,
    );

    const { result } = renderHook(() =>
      useDaemonSession('daemon-key', NEW_TARGET),
    );
    await act(async () => {
      await Promise.resolve();
    });
    act(() => {
      handlers?.onMessage(READY_MESSAGE);
      handlers?.onMessage({
        type: 'session_created',
        session_id: 'activated-session',
      });
      handlers?.onClose(new CloseEvent('close', { code: 1006 }));
    });
    expect(result.current.connectionPhase).toBe('reconnecting');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(SESSION_RECONNECT_DELAYS_MS[0]);
    });

    expect(sessionConnectionMocks.openNewSession).toHaveBeenCalledTimes(1);
    expect(sessionConnectionMocks.attachSession).toHaveBeenCalledWith(
      'daemon-key',
      'activated-session',
      expect.any(Object),
      expect.any(Object),
    );
  });

  it('reconnects a prepared session that times out before any prompt', async () => {
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

    expect(result.current.connectionPhase).toBe('reconnecting');
    expect(result.current.connectionError).toContain('timed out');
    expect(result.current.state.ready).toBe(false);
  });

  it('manually retries a failed token or upgrade attempt', async () => {
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

    act(() => result.current.retry());

    await waitFor(() => {
      expect(sessionConnectionMocks.openNewSession).toHaveBeenCalledTimes(2);
      expect(result.current.connectionPhase).toBe('preparing');
    });
  });
});
