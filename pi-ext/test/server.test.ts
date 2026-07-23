import { mkdtemp, mkdir, rm } from "node:fs/promises";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import WebSocket from "ws";

import { loadDaemonConfig } from "../src/config.js";
import { createPiDaemon, type PiDaemon } from "../src/index.js";
import type { ServerMessage } from "../src/protocol.js";

const temporaryDirectories: string[] = [];
const daemons: PiDaemon[] = [];

afterEach(async () => {
  await Promise.all(daemons.splice(0).map((daemon) => daemon.close()));
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("daemon transport", () => {
  it("authenticates HTTP, performs the one-use WS handshake, and keeps /new prepared", async (context) => {
    const root = await temporaryDirectory();
    const workspace = join(root, "workspace");
    const agentDir = join(root, "agent");
    await mkdir(workspace);
    const config = await loadDaemonConfig({
      agentDir,
      cwd: workspace,
      env: {
        PI_EXT_BIND: "127.0.0.1:0",
        PI_EXT_WORKSPACE_DIR: workspace,
      },
    });
    const daemon = await createPiDaemon({ config });
    daemons.push(daemon);
    let address: AddressInfo;
    try {
      address = await daemon.start();
    } catch (error) {
      if (isNodeError(error, "EPERM")) {
        context.skip("the execution sandbox does not permit loopback listeners");
        return;
      }
      throw error;
    }
    const baseUrl = `http://127.0.0.1:${address.port}`;
    const headers = { Authorization: `Bearer ${config.authKey}` };

    const unauthorized = await fetch(`${baseUrl}/v1/sessions`);
    expect(unauthorized.status).toBe(401);

    const provider = await fetch(`${baseUrl}/v1/provider`, {
      method: "PUT",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({
        provider: "openai_chat",
        api_key: "test-api-key",
        base_url: "http://127.0.0.1:9/v1",
        model: "test-model",
        system_prompt: "accepted but ignored",
        max_context_tokens: 16_384,
      }),
    });
    expect(provider.status).toBe(200);
    const publicProvider = await provider.text();
    expect(publicProvider).not.toContain("test-api-key");
    expect(JSON.parse(publicProvider)).toMatchObject({
      configured: true,
      provider: {
        api_key_configured: true,
        model: "test-model",
        system_prompt: null,
        max_retries: 10,
        request_timeout_secs: 30,
        stream_idle_timeout_secs: 120,
      },
    });

    const legacyAgentProfile = await fetch(`${baseUrl}/v1/agent-profiles/legacy`, {
      method: "PUT",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({ initial_agent_mode: "plan" }),
    });
    expect(legacyAgentProfile.status).toBe(200);
    const legacyAgentProfileText = await legacyAgentProfile.text();
    expect(legacyAgentProfileText).not.toContain("initial_agent_mode");

    const invalidSchedule = await fetch(`${baseUrl}/v1/scheduled-tasks`, {
      method: "POST",
      headers: { ...headers, "Content-Type": "application/json" },
      body: JSON.stringify({
        name: "invalid",
        prompt: "ignored",
        schedule: { type: "interval", every: 1, unit: "hours", extra: true },
      }),
    });
    expect(invalidSchedule.status).toBe(400);
    expect(await invalidSchedule.json()).toMatchObject({ code: "invalid_scheduled_task" });

    const tokenResponse = await fetch(`${baseUrl}/v1/auth/token`, {
      method: "POST",
      headers,
    });
    expect(tokenResponse.status).toBe(200);
    const token = (await tokenResponse.json()) as { token: string };
    const webSocket = new WebSocket(
      `ws://127.0.0.1:${address.port}/v1/ws/new`,
      ["phi.v1", `phi.auth.${token.token}`],
    );
    const messages = await collectMessages(webSocket, 2);
    expect(webSocket.protocol).toBe("phi.v1");
    expect(messages.map(({ type }) => type)).toEqual(["building", "ready"]);
    webSocket.close();
    await new Promise<void>((resolve) => webSocket.once("close", () => resolve()));

    const sessionsResponse = await fetch(`${baseUrl}/v1/sessions`, { headers });
    const sessions = (await sessionsResponse.json()) as { sessions: unknown[] };
    expect(sessions.sessions).toEqual([]);
  }, 15_000);
});

function collectMessages(webSocket: WebSocket, count: number): Promise<ServerMessage[]> {
  return new Promise((resolve, reject) => {
    const messages: ServerMessage[] = [];
    const timer = setTimeout(() => reject(new Error("timed out waiting for WebSocket messages")), 10_000);
    webSocket.on("message", (data) => {
      messages.push(JSON.parse(data.toString()) as ServerMessage);
      if (messages.length === count) {
        clearTimeout(timer);
        resolve(messages);
      }
    });
    webSocket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

async function temporaryDirectory(): Promise<string> {
  const path = await mkdtemp(join(tmpdir(), "pi-ext-server-"));
  temporaryDirectories.push(path);
  return path;
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}
