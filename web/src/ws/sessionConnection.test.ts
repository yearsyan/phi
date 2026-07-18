import { afterEach, describe, expect, it, vi } from 'vitest';
import { SessionSocket } from './connection.ts';
import { openNewSession } from './sessionConnection.ts';

const handlers = {
  onMessage: vi.fn(),
  onClose: vi.fn(),
  onError: vi.fn(),
};

describe('openNewSession', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('adds optional Agent Profile and capability mode query parameters', async () => {
    const socket = {} as SessionSocket;
    const signal = new AbortController().signal;
    const open = vi.spyOn(SessionSocket, 'open').mockResolvedValue(socket);

    await openNewSession('daemon-key', 'provider profile', handlers, {
      signal,
      agentProfileId: 'review only',
      capabilityMode: 'read_only',
      workspace: '/workspace/Project A',
    });

    expect(open).toHaveBeenCalledWith(
      '/v1/ws/new?profile_id=provider+profile&agent_profile_id=review+only&capability_mode=read_only&workspace=%2Fworkspace%2FProject+A',
      'daemon-key',
      handlers,
      { signal },
    );
  });

  it('omits optional new-session overrides when they are not configured', async () => {
    const socket = {} as SessionSocket;
    const open = vi.spyOn(SessionSocket, 'open').mockResolvedValue(socket);

    await openNewSession('daemon-key', 'default', handlers);

    expect(open).toHaveBeenCalledWith(
      '/v1/ws/new?profile_id=default',
      'daemon-key',
      handlers,
      undefined,
    );
  });
});
