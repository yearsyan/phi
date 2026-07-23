import { join } from "node:path";

import type {
  CapabilityMode,
  PublicAgentProfile,
  PutAgentProfileRequest,
  PutProviderRequest,
  ReasoningEffort,
  ScheduledTask,
} from "./protocol.js";
import { isCapabilityMode, isReasoningEffort } from "./protocol.js";
import { SecureJsonFile } from "./json-store.js";

export interface StoredProviderProfile extends PutProviderRequest {
  profile_id: string;
  revision: number;
}

export interface SessionRecord {
  session_id: string;
  session_file: string | null;
  title: string | null;
  pinned: boolean;
  profile_id: string;
  agent_profile: PublicAgentProfile;
  workspace: string;
  capability_mode: CapabilityMode;
  model_provider: string;
  model: string;
  reasoning_effort: ReasoningEffort | null;
  config_revision: number;
  created_at: string;
  updated_at: string;
}

interface ControlData {
  version: 1;
  sessions: Record<string, SessionRecord>;
  agent_profiles: Record<string, PublicAgentProfile>;
  provider_profiles: Record<string, StoredProviderProfile>;
  scheduled_tasks: Record<string, ScheduledTask>;
}

export class ControlStore {
  readonly #file: SecureJsonFile<ControlData>;

  constructor(dataDir: string) {
    this.#file = new SecureJsonFile(join(dataDir, "control.json"), emptyControlData);
  }

  async listSessions(): Promise<SessionRecord[]> {
    return Object.values((await this.#file.read()).sessions).map(clone);
  }

  async getSession(sessionId: string): Promise<SessionRecord | undefined> {
    const record = (await this.#file.read()).sessions[sessionId];
    return record === undefined ? undefined : clone(record);
  }

  async putSession(record: SessionRecord): Promise<void> {
    await this.#file.update((data) => {
      data.sessions[record.session_id] = clone(record);
    });
  }

  async updateSession(
    sessionId: string,
    update: (record: SessionRecord) => void,
  ): Promise<SessionRecord | undefined> {
    return this.#file.update((data) => {
      const record = data.sessions[sessionId];
      if (record === undefined) return undefined;
      update(record);
      record.updated_at = new Date().toISOString();
      return clone(record);
    });
  }

  async deleteSession(sessionId: string): Promise<SessionRecord | undefined> {
    return this.#file.update((data) => {
      const record = data.sessions[sessionId];
      if (record === undefined) return undefined;
      delete data.sessions[sessionId];
      return clone(record);
    });
  }

  async listAgentProfiles(): Promise<PublicAgentProfile[]> {
    const configured = Object.values((await this.#file.read()).agent_profiles).map(clone);
    if (!configured.some((profile) => profile.agent_profile_id === "default")) {
      configured.unshift(defaultAgentProfile());
    }
    return configured.sort((left, right) => left.agent_profile_id.localeCompare(right.agent_profile_id));
  }

  async getAgentProfile(id: string): Promise<PublicAgentProfile | undefined> {
    validateIdentifier("agent profile", id);
    const profile = (await this.#file.read()).agent_profiles[id];
    if (profile !== undefined) return clone(profile);
    return id === "default" ? defaultAgentProfile() : undefined;
  }

  async putAgentProfile(
    id: string,
    request: PutAgentProfileRequest,
  ): Promise<PublicAgentProfile> {
    validateIdentifier("agent profile", id);
    const normalized = normalizeAgentProfileRequest(id, request);
    return this.#file.update((data) => {
      const current = data.agent_profiles[id];
      const profile: PublicAgentProfile = {
        ...normalized,
        revision: (current?.revision ?? 0) + 1,
      };
      data.agent_profiles[id] = profile;
      return clone(profile);
    });
  }

  async listProviderProfiles(): Promise<StoredProviderProfile[]> {
    return Object.values((await this.#file.read()).provider_profiles).map(clone);
  }

  async getProviderProfile(id: string): Promise<StoredProviderProfile | undefined> {
    validateIdentifier("provider profile", id);
    const profile = (await this.#file.read()).provider_profiles[id];
    return profile === undefined ? undefined : clone(profile);
  }

  async putProviderProfile(
    id: string,
    request: PutProviderRequest,
  ): Promise<StoredProviderProfile> {
    validateIdentifier("provider profile", id);
    validateProviderRequest(request);
    return this.#file.update((data) => {
      const current = data.provider_profiles[id];
      const profile: StoredProviderProfile = {
        ...clone(request),
        profile_id: id,
        revision: (current?.revision ?? 0) + 1,
      };
      data.provider_profiles[id] = profile;
      return clone(profile);
    });
  }

  async listScheduledTasks(): Promise<ScheduledTask[]> {
    return Object.values((await this.#file.read()).scheduled_tasks).map(clone);
  }

  async getScheduledTask(id: string): Promise<ScheduledTask | undefined> {
    const task = (await this.#file.read()).scheduled_tasks[id];
    return task === undefined ? undefined : clone(task);
  }

  async putScheduledTask(task: ScheduledTask): Promise<void> {
    await this.#file.update((data) => {
      data.scheduled_tasks[task.task_id] = clone(task);
    });
  }

  async updateScheduledTask(
    id: string,
    update: (task: ScheduledTask) => void,
  ): Promise<ScheduledTask | undefined> {
    return this.#file.update((data) => {
      const task = data.scheduled_tasks[id];
      if (task === undefined) return undefined;
      update(task);
      return clone(task);
    });
  }

  async deleteScheduledTask(id: string): Promise<ScheduledTask | undefined> {
    return this.#file.update((data) => {
      const task = data.scheduled_tasks[id];
      if (task === undefined) return undefined;
      delete data.scheduled_tasks[id];
      return clone(task);
    });
  }
}

export function defaultAgentProfile(): PublicAgentProfile {
  return {
    agent_profile_id: "default",
    revision: 0,
    prompt: { mode: "extend", text: "" },
    tools: { allow: null, deny: [] },
    skills: { allow: null, deny: [] },
    initial_capability_mode: "full_access",
    model: null,
    reasoning_effort: null,
  };
}

function normalizeAgentProfileRequest(
  id: string,
  request: PutAgentProfileRequest,
): Omit<PublicAgentProfile, "revision"> {
  rejectUnknownKeys(request, [
    "prompt",
    "tools",
    "skills",
    "initial_agent_mode",
    "initial_capability_mode",
    "model",
    "reasoning_effort",
  ]);
  if (request.prompt !== undefined && !isRecord(request.prompt)) {
    throw new Error("agent profile prompt must be an object");
  }
  const prompt = request.prompt ?? {};
  rejectUnknownKeys(prompt, ["mode", "text"]);
  const mode = prompt.mode ?? "extend";
  if (prompt.text !== undefined && typeof prompt.text !== "string") {
    throw new Error("agent profile prompt text must be a string");
  }
  const text = (prompt.text ?? "").replace(/\r\n/g, "\n").trim();
  if (mode !== "extend" && mode !== "full") throw new Error("invalid prompt mode");
  if (mode === "full" && !text) throw new Error("a full prompt must not be empty");
  if (Buffer.byteLength(text) > 128 * 1024) throw new Error("agent profile prompt is too large");
  const capabilityMode = request.initial_capability_mode ?? "full_access";
  if (!isCapabilityMode(capabilityMode)) throw new Error("invalid initial capability mode");
  if (
    request.initial_agent_mode !== undefined &&
    request.initial_agent_mode !== null &&
    request.initial_agent_mode !== "default" &&
    request.initial_agent_mode !== "plan"
  ) {
    throw new Error("invalid legacy initial agent mode");
  }
  const reasoning = request.reasoning_effort ?? null;
  if (reasoning !== null && !isReasoningEffort(reasoning)) throw new Error("invalid reasoning effort");
  if (request.model !== undefined && request.model !== null && typeof request.model !== "string") {
    throw new Error("agent profile model must be a string or null");
  }
  const model = request.model?.trim() || null;
  return {
    agent_profile_id: id,
    prompt: { mode, text },
    tools: normalizeNamePolicy(request.tools),
    skills: normalizeNamePolicy(request.skills),
    initial_capability_mode: capabilityMode,
    model,
    reasoning_effort: reasoning,
  };
}

function normalizeNamePolicy(
  policy: { allow?: string[] | null; deny?: string[] } | undefined,
): { allow: string[] | null; deny: string[] } {
  if (policy === undefined) return { allow: null, deny: [] };
  if (!isRecord(policy)) throw new Error("name policy must be an object");
  rejectUnknownKeys(policy, ["allow", "deny"]);
  const allow = policy.allow === undefined || policy.allow === null ? null : normalizeNames(policy.allow);
  const deny = normalizeNames(policy.deny ?? []);
  if (allow !== null) {
    const denied = new Set(deny);
    const overlap = allow.find((name) => denied.has(name));
    if (overlap !== undefined) throw new Error(`name policy allows and denies \`${overlap}\``);
  }
  return { allow, deny };
}

function normalizeNames(names: string[]): string[] {
  if (!Array.isArray(names) || names.length > 512) throw new Error("invalid name policy");
  const normalized = names.map((name) => {
    if (typeof name !== "string") throw new Error("policy names must be strings");
    const value = name.trim();
    if (!value || Buffer.byteLength(value) > 128) throw new Error("invalid policy name");
    return value;
  });
  return [...new Set(normalized)].sort();
}

function validateProviderRequest(request: PutProviderRequest): void {
  rejectUnknownKeys(request, [
    "provider",
    "api_key",
    "base_url",
    "model",
    "system_prompt",
    "max_output_tokens",
    "max_context_tokens",
    "temperature",
    "reasoning_effort",
    "max_retries",
    "request_timeout_secs",
    "stream_idle_timeout_secs",
  ]);
  if (
    typeof request.provider !== "string" ||
    !["openai_chat", "openai_responses", "anthropic"].includes(request.provider)
  ) {
    throw new Error("unsupported provider kind");
  }
  if (typeof request.api_key !== "string") throw new Error("api_key must be a string");
  if (!request.api_key.trim()) throw new Error("api_key must not be empty");
  if (typeof request.base_url !== "string") throw new Error("base_url must be a string");
  const url = new URL(request.base_url);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("base_url must use http or https");
  }
  if (typeof request.model !== "string" || !request.model.trim()) {
    throw new Error("model must not be empty");
  }
  if (
    request.system_prompt !== undefined &&
    request.system_prompt !== null &&
    typeof request.system_prompt !== "string"
  ) {
    throw new Error("system_prompt must be a string or null");
  }
  if (!Number.isSafeInteger(request.max_context_tokens) || request.max_context_tokens <= 0) {
    throw new Error("max_context_tokens must be a positive integer");
  }
  if (
    request.max_output_tokens !== undefined &&
    request.max_output_tokens !== null &&
    (!Number.isSafeInteger(request.max_output_tokens) || request.max_output_tokens <= 0)
  ) {
    throw new Error("max_output_tokens must be a positive integer or null");
  }
  if (
    request.reasoning_effort !== undefined &&
    request.reasoning_effort !== null &&
    !isReasoningEffort(request.reasoning_effort)
  ) {
    throw new Error("invalid reasoning_effort");
  }
  if (
    request.temperature !== undefined &&
    request.temperature !== null &&
    (typeof request.temperature !== "number" || !Number.isFinite(request.temperature))
  ) {
    throw new Error("temperature must be a finite number or null");
  }
  if (
    request.max_retries !== undefined &&
    (!Number.isSafeInteger(request.max_retries) || request.max_retries < 0)
  ) {
    throw new Error("max_retries must be a non-negative integer");
  }
  if (
    request.request_timeout_secs !== undefined &&
    (!Number.isSafeInteger(request.request_timeout_secs) || request.request_timeout_secs <= 0)
  ) {
    throw new Error("request_timeout_secs must be a positive integer");
  }
  if (
    request.stream_idle_timeout_secs !== undefined &&
    (!Number.isSafeInteger(request.stream_idle_timeout_secs) ||
      request.stream_idle_timeout_secs <= 0)
  ) {
    throw new Error("stream_idle_timeout_secs must be a positive integer");
  }
}

function validateIdentifier(kind: string, id: string): void {
  const normalized = id.trim();
  if (
    normalized !== id ||
    !normalized ||
    Buffer.byteLength(normalized) > 128 ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(normalized)
  ) {
    throw new Error(`invalid ${kind} id`);
  }
}

function rejectUnknownKeys(value: object, allowed: readonly string[]): void {
  const accepted = new Set(allowed);
  const unknown = Object.keys(value).find((key) => !accepted.has(key));
  if (unknown !== undefined) throw new Error(`unknown field \`${unknown}\``);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function emptyControlData(): ControlData {
  return {
    version: 1,
    sessions: {},
    agent_profiles: {},
    provider_profiles: {},
    scheduled_tasks: {},
  };
}

function clone<T>(value: T): T {
  return structuredClone(value);
}
