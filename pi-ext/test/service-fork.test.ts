import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { AssistantMessage, ToolResultMessage, Usage, UserMessage } from "@earendil-works/pi-ai";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { afterEach, describe, expect, it } from "vitest";

import { ControlStore, defaultAgentProfile, type SessionRecord } from "../src/control-store.js";
import type { PiSessionFactory } from "../src/pi-session.js";
import { forkEntriesAt } from "../src/service.js";
import { ApplicationService } from "../src/service.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("session fork projection", () => {
  it("keeps complete tool batches after a response and strips replay state before tools", async () => {
    const root = await mkdtemp(join(tmpdir(), "pi-ext-fork-"));
    temporaryDirectories.push(root);
    const workspace = join(root, "workspace");
    const sessions = join(root, "sessions");
    await Promise.all([mkdir(workspace), mkdir(sessions)]);
    const manager = SessionManager.create(workspace, sessions, {
      id: "019f0000-0000-7000-8000-000000000001",
    });
    manager.appendMessage(user("inspect"));
    manager.appendMessage(assistantWithTool());
    manager.appendMessage(toolResult());
    manager.appendMessage(assistantDone());

    const after = forkEntriesAt(manager, 1, "after");
    expect(
      after.filter((entry) => entry.type === "message").map((entry) => entry.message.role),
    ).toEqual(["user", "assistant", "toolResult"]);

    const before = forkEntriesAt(manager, 1, "before_tool_calls");
    const messages = before.filter((entry) => entry.type === "message");
    expect(messages.map((entry) => entry.message.role)).toEqual(["user", "assistant"]);
    const sanitized = messages[1]?.message;
    expect(sanitized).toMatchObject({
      role: "assistant",
      content: [{ type: "text", text: "checking" }],
      stopReason: "stop",
      usage: { totalTokens: 0 },
    });
    expect(sanitized).not.toHaveProperty("responseId");
  });

  it("writes a durable offline fork with usage reset to zero", async () => {
    const root = await mkdtemp(join(tmpdir(), "pi-ext-fork-service-"));
    temporaryDirectories.push(root);
    const workspace = join(root, "workspace");
    const sessions = join(root, "sessions");
    await Promise.all([mkdir(workspace), mkdir(sessions)]);
    const sourceId = "019f0000-0000-7000-8000-000000000002";
    const manager = SessionManager.create(workspace, sessions, { id: sourceId });
    manager.appendMessage(user("inspect"));
    manager.appendMessage(assistantWithTool());
    manager.appendMessage(toolResult());
    const sessionFile = manager.getSessionFile();
    if (sessionFile === undefined) throw new Error("source session was not persisted");

    const store = new ControlStore(join(root, "daemon"));
    await store.putSession(record(sourceId, sessionFile, workspace));
    const service = new ApplicationService(
      store,
      {} as PiSessionFactory,
      workspace,
      join(root, "agent"),
    );

    const fork = await service.forkSession(sourceId, 1, "after");
    const forkRecord = await store.getSession(fork.session_id);
    if (forkRecord?.session_file === null || forkRecord?.session_file === undefined) {
      throw new Error("fork session was not persisted");
    }
    const forked = SessionManager.open(forkRecord.session_file);
    const assistant = forked
      .getEntries()
      .find(
        (entry) => entry.type === "message" && entry.message.role === "assistant",
      );
    expect(assistant).toMatchObject({
      type: "message",
      message: { usage: { totalTokens: 0 } },
    });
  });
});

function user(content: string): UserMessage {
  return { role: "user", content, timestamp: Date.now() };
}

function assistantWithTool(): AssistantMessage {
  return {
    role: "assistant",
    content: [
      { type: "thinking", thinking: "private", thinkingSignature: "opaque" },
      { type: "text", text: "checking", textSignature: "opaque" },
      { type: "toolCall", id: "call-1", name: "read", arguments: { path: "README.md" } },
    ],
    api: "openai-responses",
    provider: "test",
    model: "test-model",
    responseId: "opaque-response",
    usage: usage(10),
    stopReason: "toolUse",
    timestamp: Date.now(),
  };
}

function toolResult(): ToolResultMessage {
  return {
    role: "toolResult",
    toolCallId: "call-1",
    toolName: "read",
    content: [{ type: "text", text: "contents" }],
    isError: false,
    timestamp: Date.now(),
  };
}

function assistantDone(): AssistantMessage {
  return {
    role: "assistant",
    content: [{ type: "text", text: "done" }],
    api: "openai-responses",
    provider: "test",
    model: "test-model",
    usage: usage(5),
    stopReason: "stop",
    timestamp: Date.now(),
  };
}

function usage(totalTokens: number): Usage {
  return {
    input: totalTokens,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  };
}

function record(sessionId: string, sessionFile: string, workspace: string): SessionRecord {
  const now = new Date().toISOString();
  return {
    session_id: sessionId,
    session_file: sessionFile,
    title: "source",
    pinned: true,
    profile_id: "default",
    agent_profile: defaultAgentProfile(),
    workspace,
    capability_mode: "full_access",
    model_provider: "test",
    model: "test-model",
    reasoning_effort: "none",
    config_revision: 0,
    created_at: now,
    updated_at: now,
  };
}
