import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { AgentMessage, ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { Model } from "@earendil-works/pi-ai";
import type { AgentSessionEvent } from "@earendil-works/pi-coding-agent";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AskUserBroker } from "../src/ask-user.js";
import { ControlStore, defaultAgentProfile, type SessionRecord } from "../src/control-store.js";
import type { RuntimeSession } from "../src/pi-session.js";
import type { PiPrompt } from "../src/projection.js";
import { SessionActor } from "../src/session-actor.js";
import { ToolPermissionBroker } from "../src/tool-permission.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe("SessionActor", () => {
  it("serializes prompts through one FIFO and emits one global sequence", async () => {
    const root = await temporaryDirectory();
    const store = new ControlStore(root);
    const runtime = new FakeRuntime();
    const actor = new SessionActor({ runtime, store, record: record(root), initialized: false });
    const sequences: number[] = [];
    const eventTypes: string[] = [];
    actor.subscribe((envelope) => {
      sequences.push(envelope.sequence);
      eventTypes.push(envelope.event.type);
    });

    expect(() => actor.enqueue({ type: "text", value: "racing attach" })).toThrow(
      "source /new connection",
    );
    const first = actor.enqueueInitial({ type: "text", value: "first" });
    const second = actor.enqueue({ type: "text", value: "second" });
    expect(first.position).toBe(1);
    expect(second.position).toBe(2);
    await vi.waitFor(() => expect(runtime.prompts).toHaveLength(1));
    expect(actor.snapshot()).toMatchObject({
      active_run_id: first.runId,
      queued_runs: 1,
      status: "running",
    });
    await expect(actor.setCapabilityMode("read_only")).rejects.toMatchObject({
      code: "session_busy",
    });

    runtime.finishCurrent();
    await vi.waitFor(() => expect(runtime.prompts).toHaveLength(2));
    expect(actor.snapshot().active_run_id).toBe(second.runId);
    runtime.finishCurrent();
    await vi.waitFor(() => expect(actor.snapshot().status).toBe("idle"));

    expect(runtime.prompts.map(({ text }) => text)).toEqual(["first", "second"]);
    expect(sequences).toEqual(sequences.map((_, index) => index + 1));
    expect(eventTypes).toContain("session_initialized");
    expect(eventTypes.indexOf("session_initialized")).toBeLessThan(
      eventTypes.indexOf("run_queued"),
    );
    expect(eventTypes.filter((type) => type === "run_completed")).toHaveLength(2);
    await actor.close();
  });

  it("preserves the wire distinction between null and none reasoning", async () => {
    const root = await temporaryDirectory();
    const store = new ControlStore(root);
    const runtime = new FakeRuntime();
    const actor = new SessionActor({ runtime, store, record: record(root), initialized: true });

    await actor.setReasoning(null);

    expect(runtime.thinkingLevel).toBe("off");
    expect(actor.snapshot().config.reasoning_effort).toBeNull();
    expect((await store.getSession(actor.id))?.reasoning_effort).toBeNull();
    await actor.close();
  });

  it("serializes metadata mutations arriving from different transports", async () => {
    const root = await temporaryDirectory();
    const store = new ControlStore(root);
    const actor = new SessionActor({
      runtime: new FakeRuntime(),
      store,
      record: record(root),
      initialized: true,
    });

    await Promise.all([actor.setReasoning(null), actor.setPinned(true)]);

    expect(actor.record).toMatchObject({ pinned: true, reasoning_effort: null });
    expect(await store.getSession(actor.id)).toMatchObject({
      pinned: true,
      reasoning_effort: null,
    });
    await actor.close();
  });

  it("retains tool arguments through Pi's argument-less completion event", async () => {
    const root = await temporaryDirectory();
    const runtime = new FakeRuntime();
    const actor = new SessionActor({
      runtime,
      store: new ControlStore(root),
      record: record(root),
      initialized: true,
    });
    const events: import("../src/protocol.js").EventDto[] = [];
    actor.subscribe((envelope) => events.push(envelope.event));

    runtime.emit({
      type: "tool_execution_start",
      toolCallId: "call-1",
      toolName: "bash",
      args: { command: "pwd" },
    });
    runtime.emit({
      type: "tool_execution_end",
      toolCallId: "call-1",
      toolName: "bash",
      result: { content: [{ type: "text", text: "/tmp" }] },
      isError: false,
    });

    expect(events.at(-1)).toMatchObject({
      type: "tool_execution_end",
      call: { id: "call-1", name: "bash", arguments: { command: "pwd" } },
    });
    await actor.close();
  });

  it("publishes one compaction failure when Pi emits compaction_end and then rejects", async () => {
    const root = await temporaryDirectory();
    const runtime = new FakeRuntime();
    const actor = new SessionActor({
      runtime,
      store: new ControlStore(root),
      record: record(root),
      initialized: true,
    });
    const eventTypes: string[] = [];
    actor.subscribe((envelope) => eventTypes.push(envelope.event.type));
    vi.spyOn(runtime, "compact").mockImplementation(async () => {
      runtime.emit({ type: "compaction_start", reason: "manual" });
      runtime.emit({
        type: "compaction_end",
        reason: "manual",
        result: undefined,
        aborted: false,
        willRetry: false,
        errorMessage: "Compaction failed: test failure",
      });
      throw new Error("test failure");
    });

    actor.compact();
    await vi.waitFor(() => expect(actor.snapshot().status).toBe("idle"));

    expect(eventTypes.filter((type) => type === "context_compaction_failed")).toHaveLength(1);
    expect(actor.snapshot()).toMatchObject({
      context_compaction: {
        phase: "failed",
        message: "Compaction failed: test failure",
      },
    });
    expect(actor.snapshot().context_compactions).toEqual([
      expect.objectContaining({ phase: "failed", message: "Compaction failed: test failure" }),
    ]);
    await actor.close();
  });

  it("captures Pi's lazily-created transcript path from the first appended entry", async () => {
    const root = await temporaryDirectory();
    const store = new ControlStore(root);
    const runtime = new FakeRuntime();
    const actor = new SessionActor({
      runtime,
      store,
      record: record(root),
      initialized: true,
    });
    runtime.sessionFile = join(root, "session.jsonl");

    runtime.emit({
      type: "entry_appended",
      entry: {
        type: "custom",
        id: "entry-1",
        parentId: null,
        timestamp: new Date().toISOString(),
        customType: "test",
      },
    });
    await vi.waitFor(() =>
      expect(actor.record.session_file).toBe(runtime.sessionFile),
    );
    expect((await store.getSession(actor.id))?.session_file).toBe(runtime.sessionFile);
    await actor.close();
  });
});

class FakeRuntime implements RuntimeSession {
  readonly id = "00000000-0000-4000-8000-000000000001";
  sessionFile: string | undefined;
  readonly workspace: string;
  readonly messages: AgentMessage[] = [];
  get displayMessages(): readonly AgentMessage[] {
    return this.messages;
  }
  readonly model = {
    id: "test-model",
    name: "test-model",
    provider: "test-provider",
    api: "openai-completions",
    baseUrl: "http://127.0.0.1:9/v1",
    reasoning: false,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 16_384,
    maxTokens: 4_096,
  } as Model<any>;
  readonly askUser = new AskUserBroker();
  readonly permissions = new ToolPermissionBroker("full_access");
  readonly skills = [];
  readonly skillDiagnostics = [];
  readonly allAllowedTools = ["read", "bash"];
  readonly prompts: PiPrompt[] = [];
  thinkingLevel: ThinkingLevel = "off";
  isStreaming = false;
  isCompacting = false;
  #listener: ((event: AgentSessionEvent) => void) | undefined;
  #resolvePrompt: (() => void) | undefined;

  constructor() {
    this.workspace = process.cwd();
  }

  get file(): string | undefined {
    return this.sessionFile;
  }

  subscribe(listener: (event: AgentSessionEvent) => void): () => void {
    this.#listener = listener;
    return () => {
      if (this.#listener === listener) this.#listener = undefined;
    };
  }

  emit(event: AgentSessionEvent): void {
    this.#listener?.(event);
  }

  prompt(prompt: PiPrompt): Promise<void> {
    this.prompts.push(prompt);
    this.isStreaming = true;
    return new Promise((resolve) => {
      this.#resolvePrompt = () => {
        this.isStreaming = false;
        resolve();
      };
    });
  }

  finishCurrent(): void {
    const resolve = this.#resolvePrompt;
    this.#resolvePrompt = undefined;
    resolve?.();
  }

  abort(): Promise<void> {
    this.finishCurrent();
    return Promise.resolve();
  }

  compact(_instructions?: string): Promise<unknown> {
    return Promise.resolve(undefined);
  }

  abortCompaction(): void {}

  resolveModel(_requested: string): Model<any> | undefined {
    return this.model;
  }

  ensureModel(_model: Model<any>): Promise<void> {
    return Promise.resolve();
  }

  setModel(_model: Model<any>): Promise<ThinkingLevel> {
    return Promise.resolve(this.thinkingLevel);
  }

  setThinkingLevel(level: ThinkingLevel): ThinkingLevel {
    this.thinkingLevel = level;
    return level;
  }

  setCapabilityMode(mode: "read_only" | "workspace_edit" | "full_access"): void {
    this.permissions.setCapabilityMode(mode);
  }

  setTitle(_title: string): void {}

  getContextUsage(): { tokens: number | null; contextWindow: number } {
    return { tokens: 0, contextWindow: this.model.contextWindow };
  }

  getCumulativeUsage(): import("../src/protocol.js").TokenUsage {
    return { input_tokens: 0, output_tokens: 0, total_tokens: 0, cached_input_tokens: 0 };
  }

  getCompactionHistory(): import("../src/protocol.js").ContextCompactionStatus[] {
    return [];
  }

  dispose(): void {}
}

function record(workspace: string): SessionRecord {
  const now = new Date().toISOString();
  return {
    session_id: "00000000-0000-4000-8000-000000000001",
    session_file: null,
    title: null,
    pinned: false,
    profile_id: "default",
    agent_profile: defaultAgentProfile(),
    workspace,
    capability_mode: "full_access",
    model_provider: "test-provider",
    model: "test-model",
    reasoning_effort: "none",
    config_revision: 0,
    created_at: now,
    updated_at: now,
  };
}

async function temporaryDirectory(): Promise<string> {
  const path = await mkdtemp(join(tmpdir(), "pi-ext-actor-"));
  temporaryDirectories.push(path);
  return path;
}
