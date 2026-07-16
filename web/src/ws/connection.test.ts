/** @vitest-environment jsdom */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { SessionSocket, type SessionSocketHandlers } from './connection.ts';

class FakeWebSocket extends EventTarget {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static readonly instances: FakeWebSocket[] = [];

  readyState = FakeWebSocket.CONNECTING;
  readonly url: string;
  readonly protocols?: string | string[];

  constructor(url: string, protocols?: string | string[]) {
    super();
    this.url = url;
    this.protocols = protocols;
    FakeWebSocket.instances.push(this);
  }

  send(): void {}

  close(code = 1000, reason = ''): void {
    if (this.readyState === FakeWebSocket.CLOSED) return;
    this.readyState = FakeWebSocket.CLOSED;
    this.dispatchEvent(new CloseEvent('close', { code, reason }));
  }

  failBeforeOpen(code = 1006, reason = ''): void {
    this.readyState = FakeWebSocket.CLOSED;
    this.dispatchEvent(new CloseEvent('close', { code, reason }));
  }
}

const handlers: SessionSocketHandlers = {
  onMessage: vi.fn(),
  onClose: vi.fn(),
  onError: vi.fn(),
};

describe('SessionSocket.open', () => {
  beforeEach(() => {
    FakeWebSocket.instances.length = 0;
    vi.stubGlobal('WebSocket', FakeWebSocket);
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({ token: 'single-use-token' }),
      }),
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it('rejects and closes the socket when the connection is aborted', async () => {
    const controller = new AbortController();
    const opening = SessionSocket.open('/v1/ws/new', 'daemon-key', handlers, {
      signal: controller.signal,
    });

    await vi.waitFor(() => {
      expect(FakeWebSocket.instances).toHaveLength(1);
    });
    controller.abort(new Error('connection cancelled'));

    await expect(opening).rejects.toThrow('connection cancelled');
    expect(FakeWebSocket.instances[0]?.readyState).toBe(FakeWebSocket.CLOSED);
  });

  it('rejects when the socket closes before the upgrade completes', async () => {
    const opening = SessionSocket.open('/v1/ws/new', 'daemon-key', handlers);

    await vi.waitFor(() => {
      expect(FakeWebSocket.instances).toHaveLength(1);
    });
    FakeWebSocket.instances[0]?.failBeforeOpen(1006, 'upgrade closed');

    await expect(opening).rejects.toThrow('upgrade closed');
  });
});
