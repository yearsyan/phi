import { fetchWsToken } from '../api/token.ts';
import type { ClientCommand, ServerMessage } from '../types/wire.ts';

/**
 * Callbacks delivered to the owner of a {@link SessionSocket}.
 */
export interface SessionSocketHandlers {
  onMessage: (message: ServerMessage) => void;
  onClose: (event: CloseEvent) => void;
  onError: (error: Error) => void;
}

export interface SessionSocketOpenOptions {
  signal?: AbortSignal;
}

const APP_PROTOCOL = 'phi.v1';
const AUTH_PROTOCOL_PREFIX = 'phi.auth.';

/**
 * A thin wrapper over a daemon WebSocket that handles the subprotocol
 * authentication handshake and JSON command/message (de)serialization.
 *
 * A socket is created already-authenticated: it mints a single-use token from
 * the daemon key and opens the WebSocket with the fixed `phi.v1` protocol plus
 * the credential protocol `phi.auth.<token>`. The daemon only echoes `phi.v1`
 * in its handshake response; this mirrors the documented client pattern.
 */
export class SessionSocket {
  private socket: WebSocket | null = null;
  private closed = false;
  private readonly handlers: SessionSocketHandlers;

  private constructor(socket: WebSocket, handlers: SessionSocketHandlers) {
    this.socket = socket;
    this.handlers = handlers;
    socket.addEventListener('open', () => {
      /* handshake already completed by the time we get here */
    });
    socket.addEventListener('message', this.handleRawMessage);
    socket.addEventListener('close', this.handleClose);
    socket.addEventListener('error', this.handleError);
  }

  /**
   * Open an authenticated WebSocket to `path` (a path beginning with `/v1/ws`).
   * Resolves once the socket is open; rejects on auth failure or upgrade error.
   */
  static async open(
    path: string,
    authKey: string,
    handlers: SessionSocketHandlers,
    options: SessionSocketOpenOptions = {},
  ): Promise<SessionSocket> {
    throwIfAborted(options.signal);
    const issued = await fetchWsToken(authKey, options.signal);
    throwIfAborted(options.signal);
    const url = buildWsUrl(path);
    const subprotocols = [APP_PROTOCOL, AUTH_PROTOCOL_PREFIX + issued.token];

    let socket: WebSocket;
    try {
      socket = new WebSocket(url, subprotocols);
    } catch (error) {
      throw new Error(
        `Failed to open WebSocket: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    const wrapper = new SessionSocket(socket, handlers);
    try {
      await wrapper.waitUntilOpen(socket, options.signal);
      return wrapper;
    } catch (error) {
      wrapper.close();
      throw error;
    }
  }

  private waitUntilOpen(
    socket: WebSocket,
    signal?: AbortSignal,
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      if (socket.readyState === WebSocket.OPEN) {
        resolve();
        return;
      }

      let settled = false;
      const cleanup = () => {
        socket.removeEventListener('open', onOpen);
        socket.removeEventListener('error', onError);
        socket.removeEventListener('close', onClose);
        signal?.removeEventListener('abort', onAbort);
      };
      const finish = (callback: () => void) => {
        if (settled) return;
        settled = true;
        cleanup();
        callback();
      };
      const onOpen = () => {
        finish(resolve);
      };
      const onError = () => {
        finish(() =>
          reject(new Error('WebSocket upgrade was rejected by the daemon')),
        );
      };
      const onClose = (event: CloseEvent) => {
        finish(() =>
          reject(
            new Error(
              event.reason ||
                `WebSocket closed before opening (code ${event.code})`,
            ),
          ),
        );
      };
      const onAbort = () => {
        finish(() => reject(abortReason(signal)));
      };

      if (signal?.aborted) {
        onAbort();
        return;
      }
      socket.addEventListener('open', onOpen, { once: true });
      socket.addEventListener('error', onError, { once: true });
      socket.addEventListener('close', onClose, { once: true });
      signal?.addEventListener('abort', onAbort, { once: true });
    });
  }

  get isOpen(): boolean {
    return this.socket !== null && this.socket.readyState === WebSocket.OPEN;
  }

  send(command: ClientCommand): void {
    if (this.socket === null || this.socket.readyState !== WebSocket.OPEN) {
      throw new Error('WebSocket is not open');
    }
    this.socket.send(JSON.stringify(command));
  }

  close(): void {
    this.closed = true;
    const socket = this.socket;
    if (socket !== null) {
      socket.removeEventListener('message', this.handleRawMessage);
      socket.removeEventListener('close', this.handleClose);
      socket.removeEventListener('error', this.handleError);
      if (
        socket.readyState === WebSocket.OPEN ||
        socket.readyState === WebSocket.CONNECTING
      ) {
        try {
          socket.close(1000, 'client closing');
        } catch {
          /* ignore */
        }
      }
      this.socket = null;
    }
  }

  private readonly handleRawMessage = (event: MessageEvent): void => {
    if (typeof event.data !== 'string') {
      // Parent sessions only exchange UTF-8 JSON text frames.
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(event.data);
    } catch {
      return;
    }
    if (!isServerMessage(parsed)) {
      this.handlers.onError(
        new Error('Daemon sent an invalid WebSocket frame'),
      );
      return;
    }
    this.handlers.onMessage(parsed);
  };

  private readonly handleClose = (event: CloseEvent): void => {
    if (this.closed) return;
    this.handlers.onClose(event);
  };

  private readonly handleError = (event: Event): void => {
    // The browser surfaces no details on the Event; a follow-up close event
    // usually accompanies this. Still notify so the UI can react.
    if (this.closed) return;
    this.handlers.onError(
      new Error(`WebSocket error (code ${getCloseCode(this.socket)})`),
    );
    void event;
  };
}

function isServerMessage(value: unknown): value is ServerMessage {
  return (
    typeof value === 'object' &&
    value !== null &&
    'type' in value &&
    typeof value.type === 'string'
  );
}

function getCloseCode(socket: WebSocket | null): number | 'unknown' {
  if (socket === null) return 'unknown';
  // `code` is populated once the socket has begun closing. The DOM lib only
  // narrows it to exist under CLOSED, so read it defensively.
  if (
    socket.readyState === WebSocket.CLOSING ||
    socket.readyState === WebSocket.CLOSED
  ) {
    return (socket as WebSocket & { code?: number }).code ?? 'unknown';
  }
  return 'unknown';
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw abortReason(signal);
  }
}

function abortReason(signal?: AbortSignal): Error {
  return signal?.reason instanceof Error
    ? signal.reason
    : new Error('WebSocket connection was cancelled');
}

/**
 * Build an absolute WebSocket URL from a path, using the page's own origin.
 * Vite's dev proxy intercepts `/v1` upgrade requests, so relative paths work.
 */
function buildWsUrl(path: string): string {
  if (typeof location === 'undefined') {
    throw new Error('WebSocket URLs require a browser environment');
  }
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  return `${scheme}://${location.host}${path}`;
}
