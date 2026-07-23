import { existsSync } from "node:fs";
import { unlink } from "node:fs/promises";
import { join, resolve } from "node:path";

import type { AgentMessage, ThinkingLevel } from "@earendil-works/pi-agent-core";
import { clampThinkingLevel, type Model, uuidv7 } from "@earendil-works/pi-ai";
import {
  AgentSession,
  type AgentSessionEvent,
  buildSessionContext,
  createAgentSession,
  DefaultResourceLoader,
  SessionManager,
  sessionEntryToContextMessages,
  type ResourceDiagnostic,
  type Skill,
} from "@earendil-works/pi-coding-agent";

import { AskUserBroker } from "./ask-user.js";
import {
  type SessionRecord,
  type ControlStore,
  type StoredProviderProfile,
} from "./control-store.js";
import { CommandError } from "./errors.js";
import {
  ProviderManager,
  type ProfileRuntime,
  resolveModel,
} from "./provider-manager.js";
import {
  projectSkill,
  projectSkillDiagnostic,
  reasoningToThinking,
  thinkingToReasoning,
  type PiPrompt,
} from "./projection.js";
import type {
  CapabilityMode,
  ContextCompactionStatus,
  PublicAgentProfile,
  ReasoningEffort,
  SkillDiagnostic,
  SkillInvocation,
  SkillSummary,
  TokenUsage,
} from "./protocol.js";
import {
  normalizePermissionRules,
  TOOL_PERMISSION_RULES_ENTRY,
  ToolPermissionBroker,
} from "./tool-permission.js";

export interface PrepareSessionOptions {
  profileId: string;
  agentProfileId: string;
  workspace: string;
  capabilityMode?: CapabilityMode;
}

export function validateSkillInvocation(
  skills: readonly SkillSummary[],
  invocation: SkillInvocation | undefined,
): void {
  if (invocation === undefined) return;
  const name = invocation.name.trim().replace(/^\/+/, "").replace(/^skill:/, "");
  if (!name) throw new CommandError("invalid_command", "skill name must not be empty");
  const skill = skills.find((candidate) => candidate.name === name);
  if (skill === undefined || !skill.user_invocable) {
    throw new CommandError("invalid_command", `skill \`${name}\` is not available in this session`);
  }
}

export interface RuntimeSession {
  readonly id: string;
  readonly file: string | undefined;
  readonly workspace: string;
  readonly messages: readonly AgentMessage[];
  readonly displayMessages: readonly AgentMessage[];
  readonly model: Model<any>;
  readonly thinkingLevel: ThinkingLevel;
  readonly isStreaming: boolean;
  readonly isCompacting: boolean;
  readonly askUser: AskUserBroker;
  readonly permissions: ToolPermissionBroker;
  readonly skills: readonly SkillSummary[];
  readonly skillDiagnostics: readonly SkillDiagnostic[];
  readonly allAllowedTools: readonly string[];
  subscribe(listener: (event: AgentSessionEvent) => void): () => void;
  prompt(prompt: PiPrompt): Promise<void>;
  abort(): Promise<void>;
  compact(instructions?: string): Promise<unknown>;
  abortCompaction(): void;
  resolveModel(requested: string): Model<any> | undefined;
  ensureModel(model: Model<any>): Promise<void>;
  setModel(model: Model<any>): Promise<ThinkingLevel>;
  setThinkingLevel(level: ThinkingLevel): ThinkingLevel;
  setCapabilityMode(mode: CapabilityMode): void;
  setTitle(title: string): void;
  getContextUsage(): { tokens: number | null; contextWindow: number } | undefined;
  getCumulativeUsage(): TokenUsage;
  getCompactionHistory(): ContextCompactionStatus[];
  dispose(): void;
}

export class PreparedSession {
  readonly id: string;
  readonly profileId: string;
  readonly agentProfile: PublicAgentProfile;
  readonly workspace: string;
  readonly skills: readonly SkillSummary[];
  readonly skillDiagnostics: readonly SkillDiagnostic[];
  readonly #providerManager: ProviderManager;
  readonly #profileRuntime: ProfileRuntime;
  readonly #resourceLoader: DefaultResourceLoader;
  readonly #sessionDir: string;
  #model: Model<any>;
  #reasoning: ThinkingLevel;
  #reasoningEffort: ReasoningEffort | null;
  #capabilityMode: CapabilityMode;
  #configRevision = 0;
  #activated = false;

  constructor(options: {
    id: string;
    profileId: string;
    agentProfile: PublicAgentProfile;
    workspace: string;
    providerManager: ProviderManager;
    profileRuntime: ProfileRuntime;
    resourceLoader: DefaultResourceLoader;
    sessionDir: string;
    model: Model<any>;
    reasoning: ThinkingLevel;
    reasoningEffort: ReasoningEffort | null;
    capabilityMode: CapabilityMode;
    skills: SkillSummary[];
    skillDiagnostics: SkillDiagnostic[];
  }) {
    this.id = options.id;
    this.profileId = options.profileId;
    this.agentProfile = options.agentProfile;
    this.workspace = options.workspace;
    this.#providerManager = options.providerManager;
    this.#profileRuntime = options.profileRuntime;
    this.#resourceLoader = options.resourceLoader;
    this.#sessionDir = options.sessionDir;
    this.#model = options.model;
    this.#reasoning = options.reasoning;
    this.#reasoningEffort = options.reasoningEffort;
    this.#capabilityMode = options.capabilityMode;
    this.skills = options.skills;
    this.skillDiagnostics = options.skillDiagnostics;
  }

  get model(): Model<any> {
    return this.#model;
  }

  get reasoningEffort(): ReasoningEffort | null {
    return this.#reasoningEffort;
  }

  get capabilityMode(): CapabilityMode {
    return this.#capabilityMode;
  }

  get configRevision(): number {
    return this.#configRevision;
  }

  async setModel(requested: string): Promise<void> {
    this.#ensurePrepared();
    const model = resolveModel(
      this.#profileRuntime.runtime,
      this.#profileRuntime.effectiveProviderId,
      requested,
    );
    if (model === undefined) throw new Error(`model \`${requested}\` is not available`);
    await this.#providerManager.ensureModelAuth(this.#profileRuntime.runtime, model);
    this.#model = model;
    this.#reasoning = clampThinkingLevel(model, this.#reasoning);
    if (this.#reasoningEffort !== null) {
      this.#reasoningEffort = thinkingToReasoning(this.#reasoning);
    }
    this.#configRevision += 1;
  }

  setReasoning(effort: ReasoningEffort | null): void {
    this.#ensurePrepared();
    this.#reasoning = clampThinkingLevel(this.#model, reasoningToThinking(effort));
    this.#reasoningEffort = effort === null ? null : thinkingToReasoning(this.#reasoning);
    this.#configRevision += 1;
  }

  setCapabilityMode(mode: CapabilityMode): void {
    this.#ensurePrepared();
    this.#capabilityMode = mode;
  }

  async activate(): Promise<RuntimeSession> {
    this.#ensurePrepared();
    const sessionManager = SessionManager.create(this.workspace, this.#sessionDir, { id: this.id });
    try {
      const runtime = await buildRuntimeSession({
        providerManager: this.#providerManager,
        profileRuntime: this.#profileRuntime,
        resourceLoader: this.#resourceLoader,
        sessionManager,
        model: this.#model,
        reasoning: this.#reasoning,
        capabilityMode: this.#capabilityMode,
        agentProfile: this.agentProfile,
        workspace: this.workspace,
        skills: [...this.skills],
        skillDiagnostics: [...this.skillDiagnostics],
      });
      this.#activated = true;
      return runtime;
    } catch (error) {
      const sessionFile = sessionManager.getSessionFile();
      if (sessionFile !== undefined) await unlink(sessionFile).catch(() => undefined);
      throw error;
    }
  }

  toRecord(sessionFile: string | undefined): SessionRecord {
    const now = new Date().toISOString();
    return {
      session_id: this.id,
      session_file: sessionFile ?? null,
      title: null,
      pinned: false,
      profile_id: this.profileId,
      agent_profile: structuredClone(this.agentProfile),
      workspace: this.workspace,
      capability_mode: this.#capabilityMode,
      model_provider: this.#model.provider,
      model: this.#model.id,
      reasoning_effort: this.#reasoningEffort,
      config_revision: this.#configRevision,
      created_at: now,
      updated_at: now,
    };
  }

  #ensurePrepared(): void {
    if (this.#activated) throw new Error("prepared session is already activated");
  }
}

export class PiSessionFactory {
  readonly #agentDir: string;
  readonly #store: ControlStore;
  readonly #providerManager: ProviderManager;

  constructor(agentDir: string, store: ControlStore, providerManager: ProviderManager) {
    this.#agentDir = agentDir;
    this.#store = store;
    this.#providerManager = providerManager;
  }

  async prepare(options: PrepareSessionOptions): Promise<PreparedSession> {
    const agentProfile = await this.#store.getAgentProfile(options.agentProfileId);
    if (agentProfile === undefined) {
      throw new Error(`agent profile \`${options.agentProfileId}\` is not configured`);
    }
    const profileRuntime = await this.#providerManager.createRuntime(
      options.profileId,
      options.workspace,
    );
    const resourceLoader = await createResourceLoader(
      options.workspace,
      this.#agentDir,
      profileRuntime,
      agentProfile,
    );
    const model = await this.#providerManager.resolveInitialModel(profileRuntime, agentProfile.model);
    await this.#providerManager.ensureModelAuth(profileRuntime.runtime, model);
    const reasoning = initialReasoning(
      model,
      profileRuntime.storedProfile,
      agentProfile,
      profileRuntime,
    );
    const loadedSkills = resourceLoader.getSkills();
    return new PreparedSession({
      id: uuidv7(),
      profileId: options.profileId,
      agentProfile,
      workspace: options.workspace,
      providerManager: this.#providerManager,
      profileRuntime,
      resourceLoader,
      sessionDir: piSessionDir(options.workspace, this.#agentDir),
      model,
      reasoning: reasoning.thinking,
      reasoningEffort: reasoning.effort,
      capabilityMode: options.capabilityMode ?? agentProfile.initial_capability_mode,
      skills: loadedSkills.skills.map(projectSkill),
      skillDiagnostics: loadedSkills.diagnostics.map(projectSkillDiagnostic),
    });
  }

  async restore(record: SessionRecord): Promise<RuntimeSession> {
    const profileRuntime = await this.#providerManager.createRuntime(record.profile_id, record.workspace);
    const resourceLoader = await createResourceLoader(
      record.workspace,
      this.#agentDir,
      profileRuntime,
      record.agent_profile,
    );
    let model = profileRuntime.runtime.getModel(record.model_provider, record.model);
    if (model === undefined) {
      try {
        model = await this.#providerManager.resolveInitialModel(profileRuntime, record.model);
      } catch {
        model = await this.#providerManager.resolveInitialModel(profileRuntime);
      }
    }
    await this.#providerManager.ensureModelAuth(profileRuntime.runtime, model);
    const sessionDir = piSessionDir(record.workspace, this.#agentDir);
    const discoveredFile =
      record.session_file !== null && existsSync(record.session_file)
        ? record.session_file
        : (await SessionManager.list(record.workspace, sessionDir)).find(
            (session) => session.id === record.session_id,
          )?.path;
    const sessionManager =
      discoveredFile === undefined
        ? SessionManager.create(record.workspace, sessionDir, { id: record.session_id })
        : SessionManager.open(discoveredFile);
    const loadedSkills = resourceLoader.getSkills();
    return buildRuntimeSession({
      providerManager: this.#providerManager,
      profileRuntime,
      resourceLoader,
      sessionManager,
      model,
      reasoning: reasoningToThinking(record.reasoning_effort),
      capabilityMode: record.capability_mode,
      agentProfile: record.agent_profile,
      workspace: record.workspace,
      skills: loadedSkills.skills.map(projectSkill),
      skillDiagnostics: loadedSkills.diagnostics.map(projectSkillDiagnostic),
    });
  }
}

class SdkRuntimeSession implements RuntimeSession {
  readonly #session: AgentSession;
  readonly #runtime: ProfileRuntime;
  readonly #providerManager: ProviderManager;
  readonly #workspace: string;
  readonly #allAllowedTools: string[];
  readonly askUser: AskUserBroker;
  readonly permissions: ToolPermissionBroker;
  readonly skills: readonly SkillSummary[];
  readonly skillDiagnostics: readonly SkillDiagnostic[];

  constructor(options: {
    session: AgentSession;
    runtime: ProfileRuntime;
    providerManager: ProviderManager;
    workspace: string;
    askUser: AskUserBroker;
    permissions: ToolPermissionBroker;
    allAllowedTools: string[];
    skills: SkillSummary[];
    skillDiagnostics: SkillDiagnostic[];
  }) {
    this.#session = options.session;
    this.#runtime = options.runtime;
    this.#providerManager = options.providerManager;
    this.#workspace = options.workspace;
    this.askUser = options.askUser;
    this.permissions = options.permissions;
    this.#allAllowedTools = options.allAllowedTools;
    this.skills = options.skills;
    this.skillDiagnostics = options.skillDiagnostics;
  }

  get id(): string {
    return this.#session.sessionId;
  }

  get file(): string | undefined {
    return this.#session.sessionFile;
  }

  get workspace(): string {
    return this.#workspace;
  }

  get messages(): readonly AgentMessage[] {
    return this.#session.messages;
  }

  get displayMessages(): readonly AgentMessage[] {
    return this.#session.sessionManager.getBranch().flatMap((entry) =>
      entry.type === "compaction" || entry.type === "branch_summary"
        ? []
        : sessionEntryToContextMessages(entry),
    );
  }

  get model(): Model<any> {
    const model = this.#session.model;
    if (model === undefined) throw new Error("Pi session has no active model");
    return model;
  }

  get thinkingLevel(): ThinkingLevel {
    return this.#session.thinkingLevel;
  }

  get isStreaming(): boolean {
    return this.#session.isStreaming;
  }

  get isCompacting(): boolean {
    return this.#session.isCompacting;
  }

  get allAllowedTools(): readonly string[] {
    return this.#allAllowedTools;
  }

  subscribe(listener: (event: AgentSessionEvent) => void): () => void {
    return this.#session.subscribe(listener);
  }

  prompt(prompt: PiPrompt): Promise<void> {
    return this.#session.prompt(prompt.text, {
      source: "rpc",
      ...(prompt.images.length === 0 ? {} : { images: prompt.images }),
    });
  }

  abort(): Promise<void> {
    return this.#session.abort();
  }

  compact(instructions?: string): Promise<unknown> {
    return this.#session.compact(instructions);
  }

  abortCompaction(): void {
    this.#session.abortCompaction();
  }

  resolveModel(requested: string): Model<any> | undefined {
    return resolveModel(this.#runtime.runtime, this.model.provider, requested);
  }

  ensureModel(model: Model<any>): Promise<void> {
    return this.#providerManager.ensureModelAuth(this.#runtime.runtime, model);
  }

  async setModel(model: Model<any>): Promise<ThinkingLevel> {
    await this.#providerManager.ensureModelAuth(this.#runtime.runtime, model);
    if (model.provider === this.model.provider && model.id === this.model.id) {
      return this.#session.thinkingLevel;
    }
    const effective = clampThinkingLevel(model, this.#session.thinkingLevel);
    this.#session.sessionManager.appendModelChange(model.provider, model.id);
    if (effective !== this.#session.thinkingLevel) {
      this.#session.sessionManager.appendThinkingLevelChange(effective);
    }
    this.#session.agent.state.model = model;
    this.#session.agent.state.thinkingLevel = effective;
    return effective;
  }

  setThinkingLevel(level: ThinkingLevel): ThinkingLevel {
    const effective = clampThinkingLevel(this.model, level);
    if (effective === this.#session.thinkingLevel) return effective;
    this.#session.sessionManager.appendThinkingLevelChange(effective);
    this.#session.agent.state.thinkingLevel = effective;
    return effective;
  }

  setCapabilityMode(mode: CapabilityMode): void {
    this.permissions.setCapabilityMode(mode);
  }

  setTitle(title: string): void {
    this.#session.sessionManager.appendSessionInfo(title);
  }

  getContextUsage(): { tokens: number | null; contextWindow: number } | undefined {
    return this.#session.getContextUsage();
  }

  getCumulativeUsage(): TokenUsage {
    const { tokens } = this.#session.getSessionStats();
    return {
      input_tokens: tokens.input,
      output_tokens: tokens.output,
      total_tokens: tokens.total,
      cached_input_tokens: tokens.cacheRead,
    };
  }

  getCompactionHistory(): ContextCompactionStatus[] {
    const entries = this.#session.sessionManager.getEntries();
    const branch = this.#session.sessionManager.getBranch();
    const statuses: ContextCompactionStatus[] = [];
    let historyIndex = 0;
    for (const entry of branch) {
      if (entry.type === "compaction") {
        statuses.push({
          phase: "completed",
          history_index: historyIndex,
          after_message_count: buildSessionContext(entries, entry.id).messages.length,
        });
        continue;
      }
      if (entry.type === "branch_summary") continue;
      historyIndex += sessionEntryToContextMessages(entry).filter(
        (message) => !(message.role === "assistant" && message.stopReason === "aborted"),
      ).length;
    }
    return statuses;
  }

  dispose(): void {
    this.askUser.cancelAll("session was closed");
    this.permissions.cancelAll("session was closed");
    this.#session.dispose();
  }
}

async function buildRuntimeSession(options: {
  providerManager: ProviderManager;
  profileRuntime: ProfileRuntime;
  resourceLoader: DefaultResourceLoader;
  sessionManager: SessionManager;
  model: Model<any>;
  reasoning: ThinkingLevel;
  capabilityMode: CapabilityMode;
  agentProfile: PublicAgentProfile;
  workspace: string;
  skills: SkillSummary[];
  skillDiagnostics: SkillDiagnostic[];
}): Promise<RuntimeSession> {
  const askUser = new AskUserBroker();
  const permissions = new ToolPermissionBroker(options.capabilityMode, {
    workspace: options.workspace,
    rules: restoredPermissionRules(options.sessionManager.getBranch()),
    persistRules: (rules) => {
      options.sessionManager.appendCustomEntry(TOOL_PERMISSION_RULES_ENTRY, {
        rules: structuredClone(rules),
      });
    },
  });
  const { session } = await createAgentSession({
    cwd: options.workspace,
    modelRuntime: options.profileRuntime.runtime,
    model: options.model,
    thinkingLevel: options.reasoning,
    sessionManager: options.sessionManager,
    settingsManager: options.profileRuntime.settings,
    resourceLoader: options.resourceLoader,
    customTools: [askUser.createTool()],
  });
  const temperature = options.profileRuntime.storedProfile?.temperature;
  if (temperature !== undefined && temperature !== null) {
    const extensionPayloadTransform = session.agent.onPayload;
    session.agent.onPayload = async (payload, model) => {
      const transformed = (await extensionPayloadTransform?.(payload, model)) ?? payload;
      return isObject(transformed) ? { ...transformed, temperature } : transformed;
    };
  }
  const allAllowedTools = session
    .getAllTools()
    .map((tool) => tool.name)
    .filter((name) => name === "askuser" || policyAllows(options.agentProfile.tools, name));
  session.setActiveToolsByName(allAllowedTools);
  const extensionBeforeToolCall = session.agent.beforeToolCall;
  session.agent.beforeToolCall = async (context, signal) => {
    const extensionResult = await extensionBeforeToolCall?.(context, signal);
    if (extensionResult?.block) return extensionResult;
    return permissions.authorize(context, signal);
  };
  return new SdkRuntimeSession({
    session,
    runtime: options.profileRuntime,
    providerManager: options.providerManager,
    workspace: options.workspace,
    askUser,
    permissions,
    allAllowedTools,
    skills: options.skills,
    skillDiagnostics: options.skillDiagnostics,
  });
}

async function createResourceLoader(
  workspace: string,
  agentDir: string,
  runtime: ProfileRuntime,
  profile: PublicAgentProfile,
): Promise<DefaultResourceLoader> {
  const fullPrompt = profile.prompt.mode === "full" ? profile.prompt.text : undefined;
  const appended =
    profile.prompt.mode === "extend" && profile.prompt.text ? [profile.prompt.text] : undefined;
  const loader = new DefaultResourceLoader({
    cwd: workspace,
    agentDir,
    settingsManager: runtime.settings,
    ...(fullPrompt === undefined ? {} : { systemPrompt: fullPrompt }),
    ...(appended === undefined ? {} : { appendSystemPrompt: appended }),
    skillsOverride: (base) => ({
      skills: base.skills.filter((skill) => policyAllows(profile.skills, skill.name)),
      diagnostics: base.diagnostics,
    }),
  });
  await loader.reload();
  return loader;
}

export function initialReasoning(
  model: Model<any>,
  provider: StoredProviderProfile | undefined,
  agentProfile: PublicAgentProfile,
  runtime: ProfileRuntime,
): { thinking: ThinkingLevel; effort: ReasoningEffort | null } {
  const requested =
    agentProfile.reasoning_effort !== null
      ? agentProfile.reasoning_effort
      : provider !== undefined
        ? (provider.reasoning_effort ?? null)
        : thinkingToReasoning(runtime.settings.getDefaultThinkingLevel() ?? "medium");
  if (requested === null) return { thinking: "off", effort: null };
  const thinking = clampThinkingLevel(model, reasoningToThinking(requested));
  return { thinking, effort: thinkingToReasoning(thinking) };
}

function policyAllows(
  policy: { allow: readonly string[] | null; deny: readonly string[] },
  name: string,
): boolean {
  return !policy.deny.includes(name) && (policy.allow === null || policy.allow.includes(name));
}

function piSessionDir(workspace: string, agentDir: string): string {
  const canonicalWorkspace = resolve(workspace);
  const safePath = `--${canonicalWorkspace.replace(/^[/\\]/u, "").replace(/[/\\:]/gu, "-")}--`;
  return join(resolve(agentDir), "sessions", safePath);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function restoredPermissionRules(
  entries: readonly import("@earendil-works/pi-coding-agent").SessionEntry[],
): import("./protocol.js").ToolPermissionRule[] {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (entry?.type !== "custom" || entry.customType !== TOOL_PERMISSION_RULES_ENTRY) continue;
    return normalizePermissionRules(isObject(entry.data) ? entry.data.rules : undefined);
  }
  return [];
}

export type { AgentSessionEvent, ResourceDiagnostic, Skill };
