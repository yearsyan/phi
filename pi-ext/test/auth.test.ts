import type { IncomingMessage } from "node:http";

import { describe, expect, it } from "vitest";

import { AuthManager, WS_AUTH_PROTOCOL_PREFIX, WS_PROTOCOL } from "../src/auth.js";

describe("AuthManager", () => {
  it("requires one exact bearer header", () => {
    const manager = new AuthManager("a".repeat(32));
    expect(manager.authorizeRequest(request(["Authorization", `Bearer ${"a".repeat(32)}`]))).toBe(
      true,
    );
    expect(manager.authorizeRequest(request(["Authorization", `bearer ${"a".repeat(32)}`]))).toBe(
      false,
    );
    expect(
      manager.authorizeRequest(
        request([
          "Authorization",
          `Bearer ${"a".repeat(32)}`,
          "Authorization",
          `Bearer ${"a".repeat(32)}`,
        ]),
      ),
    ).toBe(false);
  });

  it("issues expiring, one-use WebSocket subprotocol tokens", () => {
    const manager = new AuthManager("b".repeat(32), 1_000);
    const issued = manager.issueToken(10_000);
    const headers = {
      "sec-websocket-protocol": `${WS_PROTOCOL}, ${WS_AUTH_PROTOCOL_PREFIX}${issued.token}`,
    };
    expect(manager.authenticateWebSocket(headers, 10_500)).toBe(true);
    expect(manager.authenticateWebSocket(headers, 10_500)).toBe(false);

    const expired = manager.issueToken(20_000);
    expect(
      manager.authenticateWebSocket(
        {
          "sec-websocket-protocol": `${WS_PROTOCOL}, ${WS_AUTH_PROTOCOL_PREFIX}${expired.token}`,
        },
        21_000,
      ),
    ).toBe(false);
  });
});

function request(rawHeaders: string[]): IncomingMessage {
  return { rawHeaders } as IncomingMessage;
}
