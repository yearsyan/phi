import type { AgentMessage, ThinkingLevel } from "@earendil-works/pi-agent-core";
import {
  clampThinkingLevel,
  uuidv7,
  type AssistantMessage,
  type ToolCall as PiToolCall,
} from "@earendil-works/pi-ai";
import type { AgentSessionEvent } from "@earendil-works/pi-coding-agent";

import type { SessionRecord, ControlStore } from "./control-store.js";
import { CommandError, errorMessage } from "./errors.js";
import { type RuntimeSession, validateSkillInvocation } from "./pi-session.js";
import {
  contentToPiPrompt,
  contextUsage,
  createDraft,
  lastUsage,
  projectAssistant,
  projectMessage,
  projectMessages,
  projectToolContent,
  projectUsage,
  reasoningToThinking,
  thinkingToReasoning,
} from "./projection.js";
import type {
  AskUserAnswer,
  AssistantDraft,
  CapabilityMode,
  Content,
  ContextCompactionStatus,
  ContextCompactionTrigger,
  EventDto,
  EventEnvelope,
  ReasoningEffort,
  SessionConfig,
  SessionDto,
  SessionStatus,
  SessionSummary,
  SkillDiagnostic,
  SkillInvocation,
  ToolCall,
  ToolPermissionDecision,
} from "./protocol.js";
import { toJsonValue } from "./protocol.js";

const PROMPT_CAPACITY = 64;

interface QueuedRun {
  id: string;
  prompt: ReturnType<typeof contentToPiPrompt>;
}

interface ActiveRun extends QueuedRun {
  stopRequested: boolean;
  messageStartIndex: number;
}

export interface QueuedRunReceipt {
  runId: string;
  position: number;
}

export type EventListener = (event: EventEnvelope) => void;

export class SessionActor {
  readonly #runtime: RuntimeSession;
  readonly #store: ControlStore;
  readonly #listeners = new Set<EventListener>();
  readonly #queue: QueuedRun[] = [];
  readonly #toolCalls = new Map<string, ToolCall>();
  readonly #unsubscribe: () => void;
  #record: SessionRecord;
  #status: SessionStatus;
  #initialized: boolean;
  #active: ActiveRun | undefined;
  #sequence = 0;
  #turn = 0;
  #draft: AssistantDraft | null = null;
  #contextCompactions: ContextCompactionStatus[] = [];
  #contextCompaction: ContextCompactionStatus | undefined;
  #compactionTrigger: ContextCompactionTrigger | undefined;
  #compactionBeforeCount = 0;
  #manualCompacting = false;
  #compactionPromise: Promise<void> | undefined;
  #closing = false;
  #pumpPromise: Promise<void> | undefined;
  #mutationTail: Promise<void> = Promise.resolve();

  constructor(options: {
    runtime: RuntimeSession;
    store: ControlStore;
    record: SessionRecord;
    initialized: boolean;
    initialSequence?: number;
  }) {
    this.#runtime = options.runtime;
    this.#store = options.store;
    this.#record = structuredClone(options.record);
    this.#initialized = options.initialized;
    this.#sequence = options.initialSequence ?? 0;
    this.#status = "idle";
    this.#contextCompactions = this.#runtime.getCompactionHistory();
    this.#contextCompaction = this.#contextCompactions.at(-1);
    this.#unsubscribe = this.#runtime.subscribe((event) => this.#onPiEvent(event));
    this.#runtime.askUser.bind({
      requested: (request) => this.#publish({ type: "askuser_requested", request }),
      answered: (askId) => this.#publish({ type: "askuser_answered", ask_id: askId }),
      cancelled: (askId) => this.#publish({ type: "askuser_cancelled", ask_id: askId }),
    });
    this.#runtime.permissions.bind({
      requested: (request) => this.#publish({ type: "tool_permission_requested", request }),
      resolved: (permissionId, allowed) =>
        this.#publish({ type: "tool_permission_resolved", permission_id: permissionId, allowed }),
      cancelled: (permissionId) =>
        this.#publish({ type: "tool_permission_cancelled", permission_id: permissionId }),
    });
  }

  get id(): string {
    return this.#record.session_id;
  }

  get record(): SessionRecord {
    return structuredClone(this.#record);
  }

  get status(): SessionStatus {
    return this.#status;
  }

  get skillDiagnostics(): readonly SkillDiagnostic[] {
    return this.#runtime.skillDiagnostics;
  }

  subscribe(listener: EventListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  snapshot(): SessionDto {
    const messages = visibleMessages(this.#runtime.messages);
    const displayMessages = visibleMessages(this.#runtime.displayMessages);
    const pendingToolPermissions = this.#runtime.permissions.listPending();
    const rawContextUsage = this.#runtime.getContextUsage();
    const projectedContextUsage = contextUsage(rawContextUsage);
    const latestUsage = lastUsage(messages);
    const awaitingPostCompactionUsage =
      rawContextUsage?.tokens === null &&
      messages.some((message) => message.role === "compactionSummary");
    const publicCompactions = [...this.#contextCompactions];
    if (
      this.#contextCompaction !== undefined &&
      !sameCompaction(publicCompactions.at(-1), this.#contextCompaction)
    ) {
      publicCompactions.push(this.#contextCompaction);
    }
    return {
      session_id: this.id,
      title: this.#record.title,
      profile_id: this.#record.profile_id,
      agent_profile: {
        agent_profile_id: this.#record.agent_profile.agent_profile_id,
        revision: this.#record.agent_profile.revision,
      },
      workspace: this.#record.workspace,
      initialized: this.#initialized,
      status: this.#status,
      active_run_id: this.#active?.id ?? null,
      queued_runs: this.#queue.length,
      capability_mode: this.#record.capability_mode,
      config: this.#config(),
      history: projectMessages(displayMessages),
      ...(publicCompactions.length === 0
        ? {}
        : { context_compactions: structuredClone(publicCompactions) }),
      ...(this.#contextCompaction === undefined
        ? {}
        : { context_compaction: structuredClone(this.#contextCompaction) }),
      draft: this.#draft === null ? null : structuredClone(this.#draft),
      pending_asks: this.#runtime.askUser.listPending(),
      ...(pendingToolPermissions.length === 0
        ? {}
        : { pending_tool_permissions: pendingToolPermissions }),
      ...(this.#runtime.skills.length === 0 ? {} : { skills: [...this.#runtime.skills] }),
      subagents: [],
      usage: {
        last: awaitingPostCompactionUsage ? null : latestUsage,
        context:
          awaitingPostCompactionUsage || latestUsage === null ? null : projectedContextUsage,
        cumulative: this.#runtime.getCumulativeUsage(),
      },
      last_sequence: this.#sequence,
    };
  }

  summary(): SessionSummary {
    return {
      session_id: this.id,
      title: this.#record.title,
      pinned: this.#record.pinned,
      profile_id: this.#record.profile_id,
      agent_profile: {
        agent_profile_id: this.#record.agent_profile.agent_profile_id,
        revision: this.#record.agent_profile.revision,
      },
      workspace: this.#record.workspace,
      status: this.#status,
      active_run_id: this.#active?.id ?? null,
      queued_runs: this.#queue.length,
      capability_mode: this.#record.capability_mode,
      config: this.#config(),
      message_count: visibleMessages(this.#runtime.displayMessages).length,
      subagents: [],
    };
  }

  enqueue(content: Content, skill?: SkillInvocation): QueuedRunReceipt {
    return this.#enqueue(content, skill, false);
  }

  enqueueInitial(content: Content, skill?: SkillInvocation): QueuedRunReceipt {
    return this.#enqueue(content, skill, true);
  }

  #enqueue(
    content: Content,
    skill: SkillInvocation | undefined,
    initialize: boolean,
  ): QueuedRunReceipt {
    this.#ensureOpen();
    if (!this.#initialized && !initialize) {
      throw new CommandError(
        "session_not_initialized",
        "the source /new connection has not admitted its first prompt",
      );
    }
    if (this.#initialized && initialize) {
      throw new CommandError("invalid_command", "the session is already initialized");
    }
    validateSkillInvocation(this.#runtime.skills, skill);
    if (this.#queue.length + (this.#active === undefined ? 0 : 1) >= PROMPT_CAPACITY) {
      throw new CommandError("queue_full", `prompt queue is full (capacity ${PROMPT_CAPACITY})`);
    }
    const id = uuidv7();
    const position = this.#queue.length + 1;
    let prompt: ReturnType<typeof contentToPiPrompt>;
    try {
      prompt = contentToPiPrompt(content, skill);
    } catch (error) {
      throw new CommandError("invalid_command", errorMessage(error));
    }
    if (initialize) {
      this.#initialized = true;
      this.#publish({ type: "session_initialized" });
    }
    this.#queue.push({ id, prompt });
    this.#publish({ type: "run_queued", run_id: id }, id);
    queueMicrotask(() => this.#startPump());
    return { runId: id, position };
  }

  stop(runId: string): void {
    this.#ensureOpen();
    if (this.#active === undefined) {
      throw new CommandError("no_active_run", `session \`${this.id}\` has no active run`);
    }
    if (this.#active.id !== runId) {
      throw new CommandError(
        "run_mismatch",
        `active run is \`${this.#active.id}\`, not \`${runId}\``,
      );
    }
    if (this.#active.stopRequested) return;
    this.#active.stopRequested = true;
    this.#runtime.askUser.cancelAll("run was stopped");
    this.#runtime.permissions.cancelAll("run was stopped");
    this.#setStatus("stopping", runId);
    void this.#runtime.abort().catch((error) => {
      this.#publish({ type: "operation_failed", operation: "stop", message: errorMessage(error) });
    });
  }

  compact(instructions?: string | null): void {
    this.#ensureConfigurable();
    this.#manualCompacting = true;
    this.#compactionTrigger = { type: "manual", instructions: instructions ?? null };
    this.#setStatus("compacting");
    const operation = this.#runtime
      .compact(instructions ?? undefined)
      .then(() => undefined)
      .catch((error) => {
        // Pi emits compaction_end before rejecting compact(). Only synthesize a
        // failure for adapters that reject without emitting the terminal event.
        if (this.#compactionTrigger !== undefined) {
          this.#finishCompactionFailure(errorMessage(error));
        }
      })
      .finally(() => {
        this.#manualCompacting = false;
        if (this.#compactionPromise === operation) this.#compactionPromise = undefined;
        if (!this.#closing && this.#status === "compacting") this.#setStatus("idle");
      });
    this.#compactionPromise = operation;
    void operation;
  }

  async setModel(requested: string): Promise<void> {
    await this.#serializeMutation(async () => {
      this.#ensureConfigurable();
      const model = this.#runtime.resolveModel(requested);
      if (model === undefined) {
        throw new CommandError("operation_failed", `model \`${requested}\` is not available`);
      }
      await this.#runtime.ensureModel(model);
      const effective = clampThinkingLevel(model, this.#runtime.thinkingLevel);
      const next = this.#nextRecord((record) => {
        record.model_provider = model.provider;
        record.model = model.id;
        if (record.reasoning_effort !== null) {
          record.reasoning_effort = thinkingToReasoning(effective);
        }
        record.config_revision += 1;
      });
      await this.#commitRecord(next, () => this.#runtime.setModel(model));
      this.#publish({ type: "config_changed", config: this.#config() });
    });
  }

  async setReasoning(effort: ReasoningEffort | null): Promise<void> {
    await this.#serializeMutation(async () => {
      this.#ensureConfigurable();
      const thinking = reasoningToThinking(effort);
      const effective = clampThinkingLevel(this.#runtime.model, thinking);
      const next = this.#nextRecord((record) => {
        record.reasoning_effort = effort === null ? null : thinkingToReasoning(effective);
        record.config_revision += 1;
      });
      await this.#commitRecord(next, () => this.#runtime.setThinkingLevel(thinking));
      this.#publish({ type: "config_changed", config: this.#config() });
    });
  }

  async setCapabilityMode(mode: CapabilityMode): Promise<void> {
    await this.#serializeMutation(async () => {
      this.#ensureConfigurable();
      const next = this.#nextRecord((record) => {
        record.capability_mode = mode;
      });
      await this.#commitRecord(next, () => this.#runtime.setCapabilityMode(mode));
      this.#publish({ type: "capability_mode_changed", capability_mode: mode });
    });
  }

  answerAskUser(askId: string, answers: AskUserAnswer[]): void {
    this.#ensureOpen();
    this.#runtime.askUser.answer(askId, answers);
  }

  decideToolPermission(permissionId: string, decision: ToolPermissionDecision): void {
    this.#ensureOpen();
    this.#runtime.permissions.decide(permissionId, decision);
  }

  disableToolPermissionPrompts(): void {
    this.#ensureOpen();
    this.#runtime.permissions.disablePrompts();
  }

  async setPinned(pinned: boolean): Promise<void> {
    await this.#serializeMutation(async () => {
      this.#ensureOpen();
      const next = this.#nextRecord((record) => {
        record.pinned = pinned;
      });
      await this.#commitRecord(next, () => undefined);
    });
  }

  async setTitle(title: string): Promise<void> {
    const normalized = title.trim();
    if (!normalized) return;
    await this.#serializeMutation(async () => {
      this.#ensureOpen();
      if (normalized === this.#record.title) return;
      const next = this.#nextRecord((record) => {
        record.title = normalized;
      });
      await this.#commitRecord(next, () => this.#runtime.setTitle(normalized));
      this.#publish({ type: "title_changed", title: normalized });
    });
  }

  async close(): Promise<void> {
    if (this.#status === "closed") return;
    this.#closing = true;
    this.#setStatus("closing");
    await this.#mutationTail;
    for (const queued of this.#queue.splice(0)) {
      this.#publish(
        { type: "run_failed", run_id: queued.id, message: "agent is closing" },
        queued.id,
      );
    }
    if (this.#active !== undefined) {
      this.#active.stopRequested = true;
      this.#runtime.askUser.cancelAll("session is closing");
      this.#runtime.permissions.cancelAll("session is closing");
      await this.#runtime.abort().catch(() => undefined);
    }
    if (this.#runtime.isCompacting) this.#runtime.abortCompaction();
    await this.#compactionPromise?.catch(() => undefined);
    await this.#pumpPromise?.catch(() => undefined);
    this.#unsubscribe();
    this.#runtime.dispose();
    this.#setStatus("closed");
  }

  #startPump(): void {
    if (this.#pumpPromise !== undefined || this.#closing || this.#manualCompacting) return;
    this.#pumpPromise = this.#pump().finally(() => {
      this.#pumpPromise = undefined;
      if (this.#queue.length > 0 && !this.#closing && !this.#manualCompacting) {
        queueMicrotask(() => this.#startPump());
      }
    });
  }

  async #pump(): Promise<void> {
    while (!this.#closing && !this.#manualCompacting) {
      const queued = this.#queue.shift();
      if (queued === undefined) return;
      const active: ActiveRun = {
        ...queued,
        stopRequested: false,
        messageStartIndex: this.#runtime.messages.length,
      };
      this.#active = active;
      this.#setStatus("running", active.id);
      this.#publish({ type: "run_started", run_id: active.id }, active.id);
      let failure: string | undefined;
      try {
        await this.#runtime.prompt(active.prompt);
      } catch (error) {
        failure = errorMessage(error);
      }
      this.#runtime.permissions.cancelAll("run finished before permission was decided");
      this.#toolCalls.clear();

      const newMessages = this.#runtime.messages.slice(active.messageStartIndex);
      const lastAssistant = [...newMessages]
        .reverse()
        .find((message): message is AssistantMessage => message.role === "assistant");
      this.#draft = null;
      if (active.stopRequested || lastAssistant?.stopReason === "aborted") {
        this.#publish({ type: "agent_stopped" }, active.id);
        this.#publish({ type: "run_stopped", run_id: active.id }, active.id);
      } else if (failure !== undefined || lastAssistant?.stopReason === "error") {
        const message = failure ?? lastAssistant?.errorMessage ?? "Pi agent run failed";
        this.#publish({ type: "run_failed", run_id: active.id, message }, active.id);
      } else {
        this.#publish({ type: "run_completed", run_id: active.id }, active.id);
      }
      this.#active = undefined;
      if (!this.#closing) this.#setStatus("idle");
    }
  }

  #onPiEvent(event: AgentSessionEvent): void {
    const runId = this.#active?.id;
    switch (event.type) {
      case "agent_start":
        this.#publish({ type: "agent_start" }, runId);
        break;
      case "agent_end":
        if (!event.willRetry) this.#publish({ type: "agent_end" }, runId);
        break;
      case "agent_settled":
      case "queue_update":
      case "auto_retry_end":
      case "summarization_retry_attempt_start":
      case "summarization_retry_finished":
        break;
      case "entry_appended":
        void this.#captureSessionFile().catch((error) =>
          this.#publish({
            type: "operation_failed",
            operation: "persist_session_file",
            message: errorMessage(error),
          }),
        );
        break;
      case "turn_start":
        this.#turn += 1;
        this.#publish({ type: "turn_start", turn: this.#turn }, runId);
        break;
      case "turn_end":
        this.#publish(
          {
            type: "turn_end",
            turn: this.#turn,
            message: projectMessage(event.message),
            tool_results: event.toolResults.map(projectMessage),
          },
          runId,
        );
        this.#draft = null;
        break;
      case "message_start":
        if (event.message.role === "assistant") this.#draft = createDraft();
        this.#publish({ type: "message_start", message: projectMessage(event.message) }, runId);
        break;
      case "message_update":
        this.#applyMessageUpdate(event);
        break;
      case "message_end":
        this.#handleMessageEnd(event.message, runId);
        break;
      case "tool_execution_start": {
        const call = piToolCall(event.toolCallId, event.toolName, event.args);
        this.#toolCalls.set(event.toolCallId, call);
        if (this.#draft !== null && this.#draft.fork_message_index === undefined) {
          this.#draft.fork_message_index = Math.max(
            0,
            visibleMessages(this.#runtime.displayMessages).length - 1,
          );
        }
        this.#publish({ type: "tool_execution_start", call }, runId);
        break;
      }
      case "tool_execution_update": {
        const call = piToolCall(event.toolCallId, event.toolName, event.args);
        this.#toolCalls.set(event.toolCallId, call);
        const result = normalizeToolResult(event.partialResult);
        this.#publish(
          {
            type: "tool_execution_progress",
            call,
            progress: {
              message: result.text,
              ...(result.metadata === undefined ? {} : { metadata: result.metadata }),
            },
          },
          runId,
        );
        break;
      }
      case "tool_execution_end": {
        const call =
          this.#toolCalls.get(event.toolCallId) ??
          piToolCall(event.toolCallId, event.toolName, undefined);
        this.#toolCalls.delete(event.toolCallId);
        const result = normalizeToolResult(event.result);
        this.#publish(
          {
            type: "tool_execution_end",
            call,
            content: result.text,
            is_error: event.isError,
            ...(result.parts.length === 0 ? {} : { content_parts: result.parts }),
            ...(result.metadata === undefined ? {} : { metadata: result.metadata }),
          },
          runId,
        );
        break;
      }
      case "compaction_start":
        this.#startCompaction(event.reason);
        break;
      case "compaction_end":
        if (event.errorMessage) {
          this.#finishCompactionFailure(event.errorMessage);
        } else if (event.aborted) {
          this.#finishCompactionFailure("context compaction was cancelled");
        } else {
          this.#finishCompactionSuccess(event.result);
        }
        break;
      case "auto_retry_start":
        this.#publish(
          {
            type: "provider_retry",
            retry_number: event.attempt,
            max_retries: event.maxAttempts,
            delay_ms: event.delayMs,
            reason: { type: "transport", message: event.errorMessage },
          },
          runId,
        );
        break;
      case "summarization_retry_scheduled":
        this.#publish({ type: "error", message: event.errorMessage }, runId);
        break;
      case "session_info_changed":
        if (event.name) void this.setTitle(event.name).catch(() => undefined);
        break;
      case "thinking_level_changed":
        void this.#serializeMutation(async () => {
          this.#ensureOpen();
          await this.#commitRecord(
            this.#nextRecord((record) => {
              record.reasoning_effort = thinkingToReasoning(event.level);
              record.config_revision += 1;
            }),
            () => undefined,
          );
        })
          .then(() => this.#publish({ type: "config_changed", config: this.#config() }))
          .catch((error) =>
            this.#publish({
              type: "operation_failed",
              operation: "set_reasoning_effort",
              message: errorMessage(error),
            }),
          );
        break;
    }
  }

  #applyMessageUpdate(event: Extract<AgentSessionEvent, { type: "message_update" }>): void {
    const update = event.assistantMessageEvent;
    if (this.#draft === null) this.#draft = createDraft();
    switch (update.type) {
      case "text_delta":
        this.#draft.text += update.delta;
        this.#publish({ type: "message_update", delta: { type: "text", delta: update.delta } }, this.#active?.id);
        break;
      case "thinking_delta":
        this.#draft.reasoning = `${this.#draft.reasoning ?? ""}${update.delta}`;
        this.#publish(
          { type: "message_update", delta: { type: "reasoning", delta: update.delta } },
          this.#active?.id,
        );
        break;
      case "toolcall_start": {
        const block = assistantToolCall(update.partial, update.contentIndex);
        this.#draft.tool_calls[update.contentIndex] = {
          index: update.contentIndex,
          id: block?.id ?? null,
          name: block?.name ?? null,
          arguments: "",
        };
        break;
      }
      case "toolcall_delta": {
        const block = assistantToolCall(update.partial, update.contentIndex);
        const draft = this.#draft.tool_calls[update.contentIndex] ?? {
          index: update.contentIndex,
          id: block?.id ?? null,
          name: block?.name ?? null,
          arguments: "",
        };
        draft.id = block?.id ?? draft.id;
        draft.name = block?.name ?? draft.name;
        draft.arguments += update.delta;
        this.#draft.tool_calls[update.contentIndex] = draft;
        this.#publish(
          {
            type: "message_update",
            delta: {
              type: "tool_call",
              index: update.contentIndex,
              id: draft.id,
              name: draft.name,
              arguments_delta: update.delta,
            },
          },
          this.#active?.id,
        );
        break;
      }
      case "toolcall_end": {
        const serialized = JSON.stringify(update.toolCall.arguments);
        this.#draft.tool_calls[update.contentIndex] = {
          index: update.contentIndex,
          id: update.toolCall.id,
          name: update.toolCall.name,
          arguments: serialized,
        };
        break;
      }
      case "start":
      case "text_start":
      case "text_end":
      case "thinking_start":
      case "thinking_end":
      case "done":
      case "error":
        break;
    }
  }

  #handleMessageEnd(message: AgentMessage, runId: string | undefined): void {
    if (message.role === "assistant" && message.stopReason === "aborted") {
      this.#draft = null;
      this.#publish({ type: "message_aborted" }, runId);
      return;
    }
    if (message.role === "assistant") {
      const projected = projectAssistant(message);
      if (projected.tool_calls.length > 0) {
        this.#draft = {
          reasoning: projected.reasoning ?? "",
          text: projected.content?.type === "text" ? projected.content.value : "",
          tool_calls: projected.tool_calls.map((call, index) => ({
            index,
            id: call.id,
            name: call.name,
            arguments: JSON.stringify(call.arguments),
          })),
          fork_message_index: Math.max(
            0,
            visibleMessages(this.#runtime.displayMessages).length - 1,
          ),
        };
      } else {
        this.#draft = null;
      }
    }
    this.#publish({ type: "message_end", message: projectMessage(message) }, runId);
    if (message.role === "assistant") {
      this.#publish(
        {
          type: "usage_update",
          usage: projectUsage(message.usage),
          context_usage: contextUsage(this.#runtime.getContextUsage()),
        },
        runId,
      );
    }
  }

  #startCompaction(reason: "manual" | "threshold" | "overflow"): void {
    const usage = contextUsage(this.#runtime.getContextUsage()) ?? {
      max_tokens: this.#runtime.model.contextWindow,
      used_tokens: 0,
      remaining_tokens: this.#runtime.model.contextWindow,
    };
    const trigger: ContextCompactionTrigger =
      this.#compactionTrigger ??
      (reason === "threshold"
        ? { type: "automatic", usage }
        : reason === "overflow"
          ? { type: "context_length_exceeded" }
          : { type: "manual", instructions: null });
    this.#compactionTrigger = trigger;
    this.#compactionBeforeCount = visibleMessages(this.#runtime.displayMessages).length;
    this.#contextCompaction = {
      phase: "started",
      history_index: this.#compactionBeforeCount,
    };
    this.#publish(
      { type: "context_compaction_started", trigger, compactor: "default" },
      this.#active?.id,
    );
  }

  #finishCompactionSuccess(result: unknown): void {
    const details = isRecord(result) ? result : {};
    const after = visibleMessages(this.#runtime.messages).length;
    const status: ContextCompactionStatus = {
      phase: "completed",
      history_index: this.#contextCompaction?.history_index ?? this.#compactionBeforeCount,
      after_message_count: after,
    };
    this.#contextCompaction = status;
    this.#contextCompactions.push(status);
    const usage = isPiUsage(details.usage) ? projectUsage(details.usage) : null;
    const estimated =
      typeof details.estimatedTokensAfter === "number"
        ? details.estimatedTokensAfter
        : contextUsage(this.#runtime.getContextUsage())?.used_tokens ?? 0;
    this.#publish(
      {
        type: "context_compaction_completed",
        trigger: this.#compactionTrigger ?? { type: "manual", instructions: null },
        compactor: "default",
        before_message_count: this.#compactionBeforeCount,
        after_message_count: after,
        usage,
        estimated_context_tokens: estimated,
      },
      this.#active?.id,
    );
    this.#compactionTrigger = undefined;
  }

  #finishCompactionFailure(message: string): void {
    const status: ContextCompactionStatus = {
      phase: "failed",
      history_index:
        this.#contextCompaction?.history_index ?? visibleMessages(this.#runtime.messages).length,
      message,
    };
    this.#contextCompaction = status;
    this.#publish(
      {
        type: "context_compaction_failed",
        trigger: this.#compactionTrigger ?? { type: "manual", instructions: null },
        compactor: "default",
        message,
      },
      this.#active?.id,
    );
    this.#compactionTrigger = undefined;
  }

  #setStatus(status: SessionStatus, runId?: string): void {
    if (this.#status === status) return;
    this.#status = status;
    this.#publish({ type: "state_changed", status }, runId);
  }

  #publish(event: EventDto, runId?: string): void {
    this.#sequence += 1;
    const envelope: EventEnvelope = {
      type: "event",
      sequence: this.#sequence,
      session_id: this.id,
      ...(runId === undefined ? {} : { run_id: runId }),
      event,
    };
    for (const listener of this.#listeners) {
      try {
        listener(envelope);
      } catch {
        // A single transport projection must not stop the owning actor.
      }
    }
  }

  #config(): SessionConfig {
    return {
      model: this.#record.model,
      reasoning_effort: this.#record.reasoning_effort,
      revision: this.#record.config_revision,
    };
  }

  #nextRecord(update: (record: SessionRecord) => void): SessionRecord {
    const next = structuredClone(this.#record);
    update(next);
    next.updated_at = new Date().toISOString();
    return next;
  }

  async #commitRecord(
    next: SessionRecord,
    apply: () => unknown | Promise<unknown>,
  ): Promise<void> {
    const previous = this.#record;
    await this.#store.putSession(next);
    try {
      await apply();
    } catch (error) {
      await this.#store.putSession(previous);
      throw error;
    }
    this.#record = next;
  }

  #captureSessionFile(): Promise<void> {
    return this.#serializeMutation(async () => {
      const sessionFile = this.#runtime.file;
      if (sessionFile === undefined || this.#record.session_file === sessionFile) return;
      const next = this.#nextRecord((record) => {
        record.session_file = sessionFile;
      });
      await this.#commitRecord(next, () => undefined);
    });
  }

  #serializeMutation<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.#mutationTail.then(operation);
    this.#mutationTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  #ensureOpen(): void {
    if (this.#closing || this.#status === "closing" || this.#status === "closed") {
      throw new CommandError("actor_stopped", `session \`${this.id}\` is closed`);
    }
  }

  #ensureConfigurable(): void {
    this.#ensureOpen();
    if (!this.#initialized) {
      throw new CommandError("session_not_initialized", "session activation is still in progress");
    }
    if (this.#status !== "idle") {
      throw new CommandError("session_busy", `session is ${this.#status}`);
    }
  }
}

function visibleMessages(messages: readonly AgentMessage[]): AgentMessage[] {
  return messages.filter(
    (message) => !(message.role === "assistant" && message.stopReason === "aborted"),
  );
}

function sameCompaction(
  left: ContextCompactionStatus | undefined,
  right: ContextCompactionStatus,
): boolean {
  return (
    left?.phase === right.phase &&
    left.history_index === right.history_index &&
    left.after_message_count === right.after_message_count &&
    left.message === right.message
  );
}

function piToolCall(id: string, name: string, argumentsValue: unknown): ToolCall {
  return { id, name, arguments: toJsonValue(argumentsValue ?? {}) };
}

function assistantToolCall(
  message: AssistantMessage,
  index: number,
): PiToolCall | undefined {
  const part = message.content[index];
  return part?.type === "toolCall" ? part : undefined;
}

function normalizeToolResult(value: unknown): {
  text: string;
  parts: ReturnType<typeof projectToolContent>["parts"];
  metadata?: ReturnType<typeof toJsonValue>;
} {
  if (!isRecord(value)) return { text: "", parts: [] };
  const content = Array.isArray(value.content)
    ? (value.content.filter(
        (part): part is { type: "text"; text: string } | { type: "image"; data: string; mimeType: string } =>
          isRecord(part) &&
          ((part.type === "text" && typeof part.text === "string") ||
            (part.type === "image" &&
              typeof part.data === "string" &&
              typeof part.mimeType === "string")),
      ) as ({ type: "text"; text: string } | { type: "image"; data: string; mimeType: string })[])
    : [];
  const projected = projectToolContent(content);
  return {
    ...projected,
    ...(value.details === undefined ? {} : { metadata: toJsonValue(value.details) }),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isPiUsage(value: unknown): value is {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number };
} {
  return (
    isRecord(value) &&
    typeof value.input === "number" &&
    typeof value.output === "number" &&
    typeof value.cacheRead === "number" &&
    typeof value.cacheWrite === "number" &&
    typeof value.totalTokens === "number" &&
    isRecord(value.cost)
  );
}

export function reasoningEffortForThinking(level: ThinkingLevel): ReasoningEffort {
  return thinkingToReasoning(level);
}
