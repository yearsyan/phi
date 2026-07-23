import { constants as fsConstants } from "node:fs";
import { access, mkdir, open, readdir, unlink } from "node:fs/promises";
import { join } from "node:path";

import { uuidv7, type AssistantMessage, type Usage as PiUsage } from "@earendil-works/pi-ai";
import {
  CURRENT_SESSION_VERSION,
  SessionManager,
  type SessionEntry,
  type SessionInfo,
  sessionEntryToContextMessages,
} from "@earendil-works/pi-coding-agent";

import {
  ControlStore,
  defaultAgentProfile,
  type SessionRecord,
} from "./control-store.js";
import { ApiError, errorMessage } from "./errors.js";
import { type PiSessionFactory, type PreparedSession } from "./pi-session.js";
import { thinkingToReasoning } from "./projection.js";
import type {
  CapabilityMode,
  Content,
  SessionSummary,
  SessionsResponse,
  SkillInvocation,
  SkillsResponse,
} from "./protocol.js";
import { SessionActor } from "./session-actor.js";
import { TOOL_PERMISSION_RULES_ENTRY } from "./tool-permission.js";

export class ApplicationService {
  readonly #store: ControlStore;
  readonly #factory: PiSessionFactory;
  readonly #defaultWorkspace: string;
  readonly #sessionsRoot: string;
  readonly #actors = new Map<string, SessionActor>();
  readonly #restoring = new Map<string, Promise<SessionActor>>();
  readonly #activating = new Set<string>();
  #shuttingDown = false;

  constructor(
    store: ControlStore,
    factory: PiSessionFactory,
    defaultWorkspace: string,
    agentDir: string,
  ) {
    this.#store = store;
    this.#factory = factory;
    this.#defaultWorkspace = defaultWorkspace;
    this.#sessionsRoot = join(agentDir, "sessions");
  }

  async prepare(options: {
    profileId?: string;
    agentProfileId?: string;
    capabilityMode?: CapabilityMode;
    workspace?: string;
  }): Promise<PreparedSession> {
    if (this.#shuttingDown) throw new Error("daemon is shutting down");
    return this.#factory.prepare({
      profileId: options.profileId ?? "default",
      agentProfileId: options.agentProfileId ?? "default",
      workspace: options.workspace ?? this.#defaultWorkspace,
      ...(options.capabilityMode === undefined ? {} : { capabilityMode: options.capabilityMode }),
    });
  }

  async activate(prepared: PreparedSession, initialSequence = 0): Promise<SessionActor> {
    if (this.#shuttingDown) throw new Error("daemon is shutting down");
    if (this.#actors.has(prepared.id) || this.#restoring.has(prepared.id)) {
      throw new Error("prepared session is already active");
    }
    this.#activating.add(prepared.id);
    const activation = this.#activatePrepared(prepared, initialSequence);
    this.#restoring.set(prepared.id, activation);
    try {
      return await activation;
    } finally {
      if (this.#restoring.get(prepared.id) === activation) {
        this.#restoring.delete(prepared.id);
      }
      this.#activating.delete(prepared.id);
    }
  }

  async #activatePrepared(
    prepared: PreparedSession,
    initialSequence: number,
  ): Promise<SessionActor> {
    const record = prepared.toRecord(undefined);
    let runtime: Awaited<ReturnType<PreparedSession["activate"]>> | undefined;
    try {
      await this.#store.putSession(record);
      runtime = await prepared.activate();
      record.session_file = runtime.file ?? null;
      record.updated_at = new Date().toISOString();
      await this.#store.putSession(record);
    } catch (error) {
      runtime?.dispose();
      if (runtime?.file !== undefined) await unlink(runtime.file).catch(() => undefined);
      await this.#store.deleteSession(prepared.id).catch(() => undefined);
      throw error;
    }
    if (runtime === undefined) throw new Error("Pi session activation did not produce a runtime");
    const actor = new SessionActor({
      runtime,
      store: this.#store,
      record,
      initialized: false,
      initialSequence,
    });
    this.#actors.set(actor.id, actor);
    return actor;
  }

  async attach(sessionId: string): Promise<SessionActor> {
    const live = this.#actors.get(sessionId);
    if (live !== undefined) return live;
    const pending = this.#restoring.get(sessionId);
    if (pending !== undefined) return pending;
    const restore = this.#restore(sessionId).finally(() => this.#restoring.delete(sessionId));
    this.#restoring.set(sessionId, restore);
    return restore;
  }

  async listSessions(): Promise<SessionsResponse> {
    const records = (await this.#allSessionRecords()).filter(
      (record) => !this.#activating.has(record.session_id),
    );
    const sessions = records
      .map((record) => this.#actors.get(record.session_id)?.summary() ?? offlineSummary(record))
      .sort((left, right) => {
        const leftPinned = records.find((record) => record.session_id === left.session_id)?.pinned ?? false;
        const rightPinned = records.find((record) => record.session_id === right.session_id)?.pinned ?? false;
        return Number(rightPinned) - Number(leftPinned) || right.session_id.localeCompare(left.session_id);
      });
    const groups = new Map<string | null, SessionSummary[]>();
    for (const session of sessions) {
      const workspace = session.workspace ?? null;
      const group = groups.get(workspace) ?? [];
      group.push(session);
      groups.set(workspace, group);
    }
    return {
      sessions,
      workspaces: [...groups].map(([workspace, grouped]) => ({ workspace, sessions: grouped })),
    };
  }

  async getSession(sessionId: string): Promise<SessionSummary> {
    const live = this.#actors.get(sessionId);
    if (live !== undefined) return live.summary();
    const record = await this.#getOrImportRecord(sessionId);
    if (record === undefined) throw new ApiError(404, "session_not_found", `session \`${sessionId}\` was not found`);
    return offlineSummary(record);
  }

  async setPinned(sessionId: string, pinned: boolean): Promise<SessionSummary> {
    const live = this.#actors.get(sessionId);
    if (live !== undefined) {
      await live.setPinned(pinned);
      return live.summary();
    }
    const record = await this.#getOrImportRecord(sessionId);
    if (record === undefined) throw new ApiError(404, "session_not_found", `session \`${sessionId}\` was not found`);
    record.pinned = pinned;
    record.updated_at = new Date().toISOString();
    await this.#store.putSession(record);
    return offlineSummary(record);
  }

  async skills(sessionId: string): Promise<SkillsResponse> {
    const actor = await this.attach(sessionId);
    const snapshot = actor.snapshot();
    return {
      session_id: sessionId,
      skills: [...(snapshot.skills ?? [])],
      ...(actor.skillDiagnostics.length === 0
        ? {}
        : { diagnostics: [...actor.skillDiagnostics] }),
    };
  }

  async deleteSession(sessionId: string): Promise<void> {
    const actor = this.#actors.get(sessionId);
    if (actor !== undefined) {
      await actor.close();
      this.#actors.delete(sessionId);
    }
    const record = await this.#getOrImportRecord(sessionId);
    if (record === undefined) throw new ApiError(404, "session_not_found", `session \`${sessionId}\` was not found`);
    await this.#store.deleteSession(sessionId);
    if (record.session_file !== null) {
      try {
        await unlink(record.session_file);
      } catch (error) {
        if (!isNodeError(error, "ENOENT")) {
          await this.#store.putSession(record);
          throw new ApiError(500, "internal_error", "session transcript could not be deleted");
        }
      }
    }
  }

  async forkSession(
    sessionId: string,
    messageIndex: number,
    position: "after" | "before_tool_calls",
  ): Promise<SessionSummary> {
    if (!Number.isSafeInteger(messageIndex) || messageIndex < 0) {
      throw new ApiError(400, "invalid_fork_point", "message_index must be a non-negative integer");
    }
    const record = await this.#getOrImportRecord(sessionId);
    if (record === undefined || record.session_file === null) {
      throw new ApiError(404, "session_not_found", `session \`${sessionId}\` was not found`);
    }
    const source = SessionManager.open(record.session_file);
    const retained = forkEntriesAt(source, messageIndex, position);
    const forkId = uuidv7();
    const forkFile = await writeForkFile({
      sourceFile: record.session_file,
      sessionDir: source.getSessionDir(),
      workspace: record.workspace,
      sessionId: forkId,
      entries: retained,
    });
    const now = new Date().toISOString();
    const forked: SessionRecord = {
      ...structuredClone(record),
      session_id: forkId,
      session_file: forkFile,
      pinned: false,
      created_at: now,
      updated_at: now,
    };
    try {
      await this.#store.putSession(forked);
    } catch (error) {
      await unlink(forkFile).catch(() => undefined);
      throw error;
    }
    return offlineSummary(forked);
  }

  scheduleTitle(actor: SessionActor, content: Content, skill?: SkillInvocation): void {
    if (actor.record.title !== null) return;
    const title = titleFromPrompt(content, skill);
    if (!title) return;
    queueMicrotask(() => void actor.setTitle(title).catch(() => undefined));
  }

  async shutdown(): Promise<void> {
    if (this.#shuttingDown) return;
    this.#shuttingDown = true;
    await Promise.allSettled([...this.#actors.values()].map((actor) => actor.close()));
    this.#actors.clear();
  }

  async #restore(sessionId: string): Promise<SessionActor> {
    if (this.#shuttingDown) throw new Error("daemon is shutting down");
    const record = await this.#getOrImportRecord(sessionId);
    if (record === undefined) throw new ApiError(404, "session_not_found", `session \`${sessionId}\` was not found`);
    const runtime = await this.#factory.restore(record);
    if (
      record.session_file !== runtime.file ||
      record.model_provider !== runtime.model.provider ||
      record.model !== runtime.model.id
    ) {
      record.session_file = runtime.file ?? null;
      record.model_provider = runtime.model.provider;
      record.model = runtime.model.id;
      await this.#store.putSession(record);
    }
    const actor = new SessionActor({ runtime, store: this.#store, record, initialized: true });
    const raced = this.#actors.get(sessionId);
    if (raced !== undefined) {
      await actor.close();
      return raced;
    }
    this.#actors.set(sessionId, actor);
    return actor;
  }

  async #allSessionRecords(): Promise<SessionRecord[]> {
    const records = await this.#store.listSessions();
    const byId = new Map(records.map((record) => [record.session_id, record]));
    for (const info of await this.#listPiSessions()) {
      if (!byId.has(info.id)) byId.set(info.id, importRecord(info, this.#defaultWorkspace));
    }
    return [...byId.values()];
  }

  async #getOrImportRecord(sessionId: string): Promise<SessionRecord | undefined> {
    const stored = await this.#store.getSession(sessionId);
    if (stored !== undefined) {
      if (stored.session_file === null) {
        const info = (await this.#listPiSessions()).find((candidate) => candidate.id === sessionId);
        if (info !== undefined) {
          stored.session_file = info.path;
          stored.updated_at = new Date().toISOString();
          await this.#store.putSession(stored);
        }
      }
      return stored;
    }
    const info = (await this.#listPiSessions()).find((candidate) => candidate.id === sessionId);
    if (info === undefined) return undefined;
    const imported = importRecord(info, this.#defaultWorkspace);
    await this.#store.putSession(imported);
    return imported;
  }

  async #listPiSessions(): Promise<SessionInfo[]> {
    let workspaceDirectories: string[] = [];
    try {
      const entries = await readdir(this.#sessionsRoot, { withFileTypes: true });
      workspaceDirectories = entries
        .filter((entry) => entry.isDirectory())
        .map((entry) => join(this.#sessionsRoot, entry.name));
    } catch (error) {
      if (!isNodeError(error, "ENOENT")) throw error;
    }
    const groups = await Promise.all([
      SessionManager.listAll(this.#sessionsRoot),
      ...workspaceDirectories.map((directory) => SessionManager.listAll(directory)),
    ]);
    const byPath = new Map<string, SessionInfo>();
    for (const info of groups.flat()) byPath.set(info.path, info);
    return [...byPath.values()];
  }
}

function offlineSummary(record: SessionRecord): SessionSummary {
  return {
    session_id: record.session_id,
    title: record.title,
    pinned: record.pinned,
    profile_id: record.profile_id,
    agent_profile: {
      agent_profile_id: record.agent_profile.agent_profile_id,
      revision: record.agent_profile.revision,
    },
    workspace: record.workspace,
    status: "offline",
    active_run_id: null,
    queued_runs: 0,
    capability_mode: null,
    config: {
      model: record.model,
      reasoning_effort: record.reasoning_effort,
      revision: record.config_revision,
    },
    message_count: null,
    subagents: [],
  };
}

function importRecord(info: SessionInfo, defaultWorkspace: string): SessionRecord {
  const manager = SessionManager.open(info.path);
  const context = manager.buildSessionContext();
  const now = new Date().toISOString();
  const thinking = isThinkingLevel(context.thinkingLevel) ? context.thinkingLevel : "off";
  return {
    session_id: info.id,
    session_file: info.path,
    title: info.name ?? null,
    pinned: false,
    profile_id: "default",
    agent_profile: defaultAgentProfile(),
    workspace: info.cwd || defaultWorkspace,
    capability_mode: "full_access",
    model_provider: context.model?.provider ?? "",
    model: context.model?.modelId ?? "",
    reasoning_effort: thinkingToReasoning(thinking),
    config_revision: 0,
    created_at: Number.isNaN(info.created.getTime()) ? now : info.created.toISOString(),
    updated_at: Number.isNaN(info.modified.getTime()) ? now : info.modified.toISOString(),
  };
}

export function forkEntriesAt(
  source: SessionManager,
  messageIndex: number,
  position: "after" | "before_tool_calls",
): SessionEntry[] {
  const contextEntries = source.buildContextEntries();
  const messages = source
    .getBranch()
    .filter((entry) => entry.type !== "compaction" && entry.type !== "branch_summary")
    .flatMap((entry) =>
      sessionEntryToContextMessages(entry)
        .filter(
          (message) => !(message.role === "assistant" && message.stopReason === "aborted"),
        )
        .map((message) => ({ entry, message })),
    );
  const selected = messages[messageIndex];
  if (selected?.message.role !== "assistant" || selected.entry.type !== "message") {
    throw new ApiError(
      400,
      "invalid_fork_point",
      "fork point must be a public assistant message",
    );
  }
  const toolCalls = selected.message.content.filter((part) => part.type === "toolCall");
  if (position === "before_tool_calls" && toolCalls.length === 0) {
    throw new ApiError(400, "invalid_fork_point", "assistant message has no tool calls");
  }

  let boundaryEntry: SessionEntry = selected.entry;
  for (let index = 0; index < toolCalls.length; index += 1) {
    const call = toolCalls[index];
    const result = messages[messageIndex + index + 1];
    if (
      call === undefined ||
      result?.message.role !== "toolResult" ||
      result.message.toolCallId !== call.id
    ) {
      throw new ApiError(
        400,
        "invalid_fork_point",
        "assistant tool-call batch is not followed by its ordered results",
      );
    }
    boundaryEntry = result.entry;
  }

  const selectedEntryIndex = contextEntries.findIndex((entry) => entry.id === selected.entry.id);
  const boundaryIndex = contextEntries.findIndex((entry) => entry.id === boundaryEntry.id);
  if (selectedEntryIndex < 0 || boundaryIndex < selectedEntryIndex) {
    throw new ApiError(400, "invalid_fork_point", "fork point is not on the active branch");
  }
  let retained = contextEntries.slice(
    0,
    position === "after" ? boundaryIndex + 1 : selectedEntryIndex,
  );
  if (position === "before_tool_calls") {
    const sanitized = sanitizeAssistantBeforeTools(selected.message);
    if (sanitized.content.length > 0) {
      retained.push({ ...structuredClone(selected.entry), message: sanitized });
    }
  }
  return retained.filter(
    (entry) => entry.type !== "custom" || entry.customType !== TOOL_PERMISSION_RULES_ENTRY,
  );
}

async function writeForkFile(options: {
  sourceFile: string;
  sessionDir: string;
  workspace: string;
  sessionId: string;
  entries: SessionEntry[];
}): Promise<string> {
  await mkdir(options.sessionDir, { recursive: true, mode: 0o700 });
  const timestamp = new Date().toISOString();
  const fileTimestamp = timestamp.replace(/[:.]/g, "-");
  const path = join(options.sessionDir, `${fileTimestamp}_${options.sessionId}.jsonl`);
  const handle = await open(path, "wx", 0o600);
  try {
    const header = {
      type: "session",
      version: CURRENT_SESSION_VERSION,
      id: options.sessionId,
      timestamp,
      cwd: options.workspace,
      parentSession: options.sourceFile,
    };
    await handle.writeFile(`${JSON.stringify(header)}\n`, "utf8");
    let parentId: string | null = null;
    for (let index = 0; index < options.entries.length; index += 1) {
      const original = options.entries[index];
      if (original === undefined || original.type === "label") continue;
      let entry = structuredClone(original);
      if (entry.type === "message" && entry.message.role === "assistant") {
        entry.message = { ...entry.message, usage: zeroPiUsage() };
      } else if (entry.type === "message" && entry.message.role === "toolResult") {
        const { usage: _usage, ...message } = entry.message;
        entry.message = message;
      } else if (entry.type === "compaction" || entry.type === "branch_summary") {
        const { usage: _usage, ...withoutUsage } = entry;
        entry = withoutUsage;
      }
      entry = { ...entry, parentId };
      await handle.writeFile(`${JSON.stringify(entry)}\n`, "utf8");
      parentId = entry.id;
    }
    await handle.sync();
  } finally {
    await handle.close();
  }
  if (process.platform !== "win32") {
    const { chmod } = await import("node:fs/promises");
    await chmod(path, 0o600);
  }
  await access(path, fsConstants.R_OK);
  return path;
}

function sanitizeAssistantBeforeTools(message: AssistantMessage): AssistantMessage {
  const { responseId: _responseId, ...withoutResponseId } = message;
  return {
    ...withoutResponseId,
    content: message.content
      .filter((part) => part.type === "text")
      .map((part) => {
        const { textSignature: _signature, ...text } = part;
        return text;
      }),
    stopReason: "stop",
    usage: zeroPiUsage(),
  };
}

function zeroPiUsage(): PiUsage {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  };
}

function titleFromPrompt(content: Content, skill?: SkillInvocation): string {
  const source =
    skill === undefined
      ? content.type === "text"
        ? content.value
        : content.value
            .filter((part) => part.type === "text")
            .map((part) => (part.type === "text" ? part.text : ""))
            .join(" ")
      : `/${skill.name.replace(/^\/+/, "")} ${skill.arguments ?? ""}`;
  return source.replace(/\s+/g, " ").trim().slice(0, 80);
}

function isThinkingLevel(value: string): value is "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" {
  return ["off", "minimal", "low", "medium", "high", "xhigh", "max"].includes(value);
}

function isNodeError(error: unknown, code: string): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === code;
}

export function serviceError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;
  return new ApiError(500, "internal_error", errorMessage(error));
}
