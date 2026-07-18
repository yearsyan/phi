import { afterEach, describe, expect, it, vi } from 'vitest';
import { browseWorkspace, forkSession, runScheduledTask } from './http.ts';

describe('browseWorkspace', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('encodes the path and authenticates the directory request', async () => {
    const fetch = vi.fn().mockResolvedValue({
      status: 200,
      ok: true,
      json: vi.fn().mockResolvedValue({
        path: '/workspace/Project A',
        parent: '/workspace',
        directories: [],
        truncated: false,
      }),
    });
    vi.stubGlobal('fetch', fetch);

    await browseWorkspace('daemon-key', '/workspace/Project A');

    expect(fetch).toHaveBeenCalledWith(
      '/v1/workspaces/browse?path=%2Fworkspace%2FProject+A',
      expect.objectContaining({
        method: 'GET',
        headers: expect.objectContaining({
          Authorization: 'Bearer daemon-key',
        }),
      }),
    );
  });
});

describe('forkSession', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('posts the provider-safe transcript index to the source session', async () => {
    const response = {
      session_id: 'forked-session',
      title: 'Fork',
      pinned: false,
      profile_id: 'default',
      agent_profile: { agent_profile_id: 'default', revision: 0 },
      workspace: '/workspace/phi',
      status: 'offline',
      active_run_id: null,
      queued_runs: 0,
      capability_mode: null,
      config: { model: 'test', reasoning_effort: null, revision: 0 },
      message_count: null,
      subagents: [],
    };
    const fetch = vi.fn().mockResolvedValue({
      status: 201,
      ok: true,
      json: vi.fn().mockResolvedValue(response),
    });
    vi.stubGlobal('fetch', fetch);

    await expect(forkSession('daemon-key', 'source/id', 7)).resolves.toEqual(
      response,
    );
    expect(fetch).toHaveBeenCalledWith(
      '/v1/sessions/source%2Fid/fork',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ message_index: 7 }),
        headers: expect.objectContaining({
          Authorization: 'Bearer daemon-key',
          'content-type': 'application/json',
        }),
      }),
    );
  });

  it('posts the before-tool-calls boundary for an intermediate fork', async () => {
    const fetch = vi.fn().mockResolvedValue({
      status: 201,
      ok: true,
      json: vi.fn().mockResolvedValue({ session_id: 'forked-session' }),
    });
    vi.stubGlobal('fetch', fetch);

    await forkSession('daemon-key', 'source-session', 9, 'before_tool_calls');

    expect(fetch).toHaveBeenCalledWith(
      '/v1/sessions/source-session/fork',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          message_index: 9,
          position: 'before_tool_calls',
        }),
      }),
    );
  });
});

describe('runScheduledTask', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('accepts the daemon 202 response without trying to parse an empty body', async () => {
    const json = vi.fn();
    const fetch = vi.fn().mockResolvedValue({
      status: 202,
      ok: true,
      json,
    });
    vi.stubGlobal('fetch', fetch);

    await expect(
      runScheduledTask('daemon-key', 'task/id'),
    ).resolves.toBeUndefined();
    expect(fetch).toHaveBeenCalledWith(
      '/v1/scheduled-tasks/task%2Fid/run',
      expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({
          Authorization: 'Bearer daemon-key',
        }),
      }),
    );
    expect(json).not.toHaveBeenCalled();
  });
});
