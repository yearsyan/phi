import { randomBytes, timingSafeEqual } from "node:crypto";
import type { IncomingHttpHeaders, IncomingMessage } from "node:http";

import type { AuthTokenResponse } from "./protocol.js";

export const WS_PROTOCOL = "phi.v1";
export const WS_AUTH_PROTOCOL_PREFIX = "phi.auth.";
const MAX_PENDING_TOKENS = 4096;

export class AuthManager {
  readonly #key: Buffer;
  readonly #ttlMs: number;
  readonly #tokens = new Map<string, number>();

  constructor(key: string, ttlMs = 60_000) {
    if (!key || ttlMs <= 0) throw new Error("auth key and token TTL must be non-empty");
    this.#key = Buffer.from(key);
    this.#ttlMs = ttlMs;
  }

  authorizeRequest(request: IncomingMessage): boolean {
    const values: string[] = [];
    for (let index = 0; index < request.rawHeaders.length; index += 2) {
      if (request.rawHeaders[index]?.toLowerCase() === "authorization") {
        const value = request.rawHeaders[index + 1];
        if (value !== undefined) values.push(value);
      }
    }
    if (values.length !== 1) return false;
    const value = values[0];
    if (value === undefined || !value.startsWith("Bearer ")) return false;
    const presented = value.slice("Bearer ".length);
    if (!presented || /\s/.test(presented)) return false;
    const bytes = Buffer.from(presented);
    return bytes.length === this.#key.length && timingSafeEqual(bytes, this.#key);
  }

  issueToken(now = Date.now()): AuthTokenResponse {
    this.#prune(now);
    if (this.#tokens.size >= MAX_PENDING_TOKENS) {
      throw new Error("too many pending WebSocket tokens");
    }
    let token: string;
    do token = randomBytes(32).toString("base64url");
    while (this.#tokens.has(token));
    this.#tokens.set(token, now + this.#ttlMs);
    return {
      token,
      token_type: "websocket_subprotocol",
      protocol: WS_PROTOCOL,
      expires_in_secs: Math.floor(this.#ttlMs / 1000),
    };
  }

  authenticateWebSocket(headers: IncomingHttpHeaders, now = Date.now()): boolean {
    const offered = parseProtocols(headers["sec-websocket-protocol"]);
    if (offered === null) return false;
    let supportsProtocol = false;
    let token: string | undefined;
    for (const protocol of offered) {
      if (protocol === WS_PROTOCOL) {
        supportsProtocol = true;
      } else if (protocol.startsWith(WS_AUTH_PROTOCOL_PREFIX)) {
        if (token !== undefined) return false;
        token = protocol.slice(WS_AUTH_PROTOCOL_PREFIX.length);
        if (!token) return false;
      }
    }
    if (!supportsProtocol || token === undefined || token.length !== 43) return false;
    this.#prune(now);
    const expiry = this.#tokens.get(token);
    this.#tokens.delete(token);
    return expiry !== undefined && expiry > now;
  }

  #prune(now: number): void {
    for (const [token, expiry] of this.#tokens) {
      if (expiry <= now) this.#tokens.delete(token);
    }
  }
}

function parseProtocols(value: string | string[] | undefined): string[] | null {
  if (value === undefined) return [];
  const values = Array.isArray(value) ? value : [value];
  const protocols: string[] = [];
  for (const item of values) {
    for (const protocol of item.split(",")) {
      const normalized = protocol.trim();
      if (!normalized) return null;
      protocols.push(normalized);
    }
  }
  return protocols;
}
