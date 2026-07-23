import { uuidv7 } from "@earendil-works/pi-ai";
import { ControlStore } from "./control-store.js";
import { ApiError, errorMessage } from "./errors.js";
import type {
  CapabilityMode,
  CreateScheduledTaskRequest,
  ScheduledTask,
  ScheduledTaskRun,
  ScheduledTaskSchedule,
  ScheduledWeekday,
  UpdateScheduledTaskRequest,
} from "./protocol.js";
import { isCapabilityMode, isRecord } from "./protocol.js";
import type { ProviderManager } from "./provider-manager.js";
import type { ApplicationService } from "./service.js";

const MAX_SCHEDULED_TASKS = 1_000;
const MAX_ACTIVE_RUNS = 8;
const MAX_TIMER_DELAY = 2_147_000_000;
const MAX_NAME_CHARS = 100;
const MAX_PROMPT_CHARS = 20_000;
const MAX_INTERVAL_SECONDS = 10 * 366 * 24 * 60 * 60;

export class ScheduledTaskManager {
  readonly #store: ControlStore;
  readonly #service: ApplicationService;
  readonly #providers: ProviderManager;
  readonly #defaultWorkspace: string;
  readonly #timers = new Map<string, NodeJS.Timeout>();
  readonly #running = new Set<string>();
  #creationTail: Promise<void> = Promise.resolve();
  #closed = false;

  constructor(
    store: ControlStore,
    service: ApplicationService,
    providers: ProviderManager,
    defaultWorkspace: string,
  ) {
    this.#store = store;
    this.#service = service;
    this.#providers = providers;
    this.#defaultWorkspace = defaultWorkspace;
  }

  async start(): Promise<void> {
    for (const task of await this.#store.listScheduledTasks()) {
      if (task.last_run?.outcome === "running") {
        task.last_run = {
          ...task.last_run,
          outcome: "interrupted",
          finished_at: new Date().toISOString(),
          error: "daemon restarted while the task was running",
        };
        task.next_run_at = task.enabled ? nextRun(task.schedule, new Date()).toISOString() : null;
        task.revision += 1;
        await this.#store.putScheduledTask(task);
      }
      if (task.enabled) this.#schedule(task);
    }
  }

  async list(): Promise<ScheduledTask[]> {
    return (await this.#store.listScheduledTasks()).sort((left, right) =>
      right.created_at.localeCompare(left.created_at),
    );
  }

  async get(taskId: string): Promise<ScheduledTask> {
    const task = await this.#store.getScheduledTask(taskId);
    if (task === undefined) throw notFound(taskId);
    return task;
  }

  async create(request: CreateScheduledTaskRequest): Promise<ScheduledTask> {
    const normalized = normalizeCreateRequest(request, this.#defaultWorkspace);
    const [provider, agentProfile] = await Promise.all([
      this.#providers.get(normalized.profile_id),
      this.#store.getAgentProfile(normalized.agent_profile_id),
    ]).catch((error) => {
      throw invalid(errorMessage(error));
    });
    if (!provider.configured || provider.provider?.api_key_configured !== true) {
      throw invalid(`Provider profile \`${normalized.profile_id}\` was not found or authenticated`);
    }
    if (agentProfile === undefined) {
      throw invalid(`Agent Profile \`${normalized.agent_profile_id}\` was not found`);
    }
    const now = new Date();
    const task: ScheduledTask = {
      task_id: uuidv7(),
      name: normalized.name,
      prompt: normalized.prompt,
      workspace: normalized.workspace,
      profile_id: normalized.profile_id,
      agent_profile_id: normalized.agent_profile_id,
      capability_mode: normalized.capability_mode,
      schedule: normalized.schedule,
      enabled: true,
      created_at: now.toISOString(),
      updated_at: now.toISOString(),
      next_run_at: nextRun(normalized.schedule, now).toISOString(),
      last_run: null,
      skipped_runs: 0,
      revision: 1,
    };
    await this.#createTask(task);
    this.#schedule(task);
    return task;
  }

  async update(taskId: string, request: UpdateScheduledTaskRequest): Promise<ScheduledTask> {
    if (typeof request.enabled !== "boolean") {
      throw new ApiError(400, "invalid_scheduled_task", "enabled must be a boolean");
    }
    if (
      request.expected_revision !== undefined &&
      (!Number.isSafeInteger(request.expected_revision) || request.expected_revision < 0)
    ) {
      throw new ApiError(
        400,
        "invalid_scheduled_task",
        "expected_revision must be a non-negative integer",
      );
    }
    const task = await this.#store.updateScheduledTask(taskId, (current) => {
      if (
        request.expected_revision !== undefined &&
        request.expected_revision !== current.revision
      ) {
        throw new ApiError(
          409,
          "scheduled_task_revision_conflict",
          `expected revision ${request.expected_revision}, current revision is ${current.revision}`,
        );
      }
      current.enabled = request.enabled;
      current.updated_at = new Date().toISOString();
      current.next_run_at = request.enabled ? nextRun(current.schedule, new Date()).toISOString() : null;
      current.revision += 1;
    });
    if (task === undefined) throw notFound(taskId);
    this.#cancelTimer(taskId);
    if (task.enabled) this.#schedule(task);
    return task;
  }

  async delete(taskId: string): Promise<void> {
    if (this.#running.has(taskId)) {
      throw new ApiError(409, "scheduled_task_already_running", "scheduled task is running");
    }
    const deleted = await this.#store.deleteScheduledTask(taskId);
    if (deleted === undefined) throw notFound(taskId);
    this.#cancelTimer(taskId);
  }

  async runNow(taskId: string): Promise<void> {
    const task = await this.get(taskId);
    this.#admit(task, new Date());
  }

  async close(): Promise<void> {
    this.#closed = true;
    for (const timer of this.#timers.values()) clearTimeout(timer);
    this.#timers.clear();
    const finishedAt = new Date().toISOString();
    await Promise.allSettled(
      [...this.#running].map((taskId) =>
        this.#store.updateScheduledTask(taskId, (current) => {
          if (current.last_run?.outcome !== "running") return;
          current.last_run = {
            ...current.last_run,
            outcome: "interrupted",
            finished_at: finishedAt,
            error: "daemon shut down before the run completed",
          };
          current.updated_at = finishedAt;
          current.revision += 1;
        }),
      ),
    );
  }

  #schedule(task: ScheduledTask): void {
    if (this.#closed || !task.enabled) return;
    this.#cancelTimer(task.task_id);
    const due = task.next_run_at ? new Date(task.next_run_at) : nextRun(task.schedule, new Date());
    const delay = Math.max(0, due.getTime() - Date.now());
    const timer = setTimeout(() => {
      this.#timers.delete(task.task_id);
      if (delay > MAX_TIMER_DELAY) {
        void this.get(task.task_id)
          .then((fresh) => this.#schedule(fresh))
          .catch(() => undefined);
        return;
      }
      try {
        this.#admit(task, due, true);
      } catch {
        // Scheduled capacity conflicts are persisted as skipped runs below.
      }
    }, Math.min(delay, MAX_TIMER_DELAY));
    timer.unref();
    this.#timers.set(task.task_id, timer);
  }

  #admit(task: ScheduledTask, scheduledFor: Date, scheduled = false): void {
    if (this.#closed) return;
    if (this.#running.has(task.task_id)) {
      if (scheduled) void this.#markSkipped(task).catch(() => undefined);
      throw new ApiError(409, "scheduled_task_already_running", "scheduled task is already running");
    }
    if (this.#running.size >= MAX_ACTIVE_RUNS) {
      if (scheduled) void this.#markSkipped(task).catch(() => undefined);
      throw new ApiError(429, "scheduled_task_capacity", "scheduled task run capacity is full");
    }
    this.#running.add(task.task_id);
    void this.#run(task, scheduledFor, scheduled)
      .catch(() => undefined)
      .finally(() => this.#running.delete(task.task_id));
  }

  async #run(task: ScheduledTask, scheduledFor: Date, scheduled: boolean): Promise<void> {
    const started = new Date();
    const run: ScheduledTaskRun = {
      scheduled_for: scheduledFor.toISOString(),
      started_at: started.toISOString(),
      finished_at: null,
      outcome: "running",
      session_id: null,
      error: null,
    };
    let admitted = false;
    const startedTask = await this.#store.updateScheduledTask(task.task_id, (current) => {
      if (scheduled && !current.enabled) return;
      admitted = true;
      current.last_run = structuredClone(run);
      if (scheduled) {
        current.next_run_at = advancePast(current.schedule, scheduledFor, started).toISOString();
      }
      current.updated_at = started.toISOString();
      current.revision += 1;
    });
    if (startedTask === undefined || !admitted) return;
    task = startedTask;

    try {
      const prepared = await this.#service.prepare({
        profileId: task.profile_id,
        agentProfileId: task.agent_profile_id,
        workspace: task.workspace,
        ...(task.capability_mode === null ? {} : { capabilityMode: task.capability_mode }),
      });
      const actor = await this.#service.activate(prepared);
      actor.disableToolPermissionPrompts();
      await actor.setTitle(task.name);
      run.session_id = actor.id;
      let linked = false;
      await this.#store.updateScheduledTask(task.task_id, (current) => {
        if (
          current.last_run?.outcome !== "running" ||
          current.last_run.started_at !== run.started_at
        ) {
          return;
        }
        linked = true;
        current.last_run.session_id = actor.id;
        current.updated_at = new Date().toISOString();
        current.revision += 1;
      });
      if (!linked) throw new Error("scheduled task run was interrupted before prompt admission");
      const receipt = actor.enqueueInitial({ type: "text", value: task.prompt });
      const outcome = await waitForRun(actor, receipt.runId);
      run.outcome = outcome.outcome;
      run.error = outcome.error;
    } catch (error) {
      run.outcome = "failed";
      run.error = errorMessage(error);
    }
    run.finished_at = new Date().toISOString();
    const updated = await this.#store.updateScheduledTask(task.task_id, (current) => {
      if (
        current.last_run?.outcome !== "running" ||
        current.last_run.started_at !== run.started_at
      ) {
        return;
      }
      current.last_run = run;
      current.updated_at = run.finished_at ?? new Date().toISOString();
      if (!current.enabled) current.next_run_at = null;
      if (current.enabled && current.next_run_at === null) {
        current.next_run_at = nextRun(current.schedule, new Date()).toISOString();
      }
      current.revision += 1;
    });
    if (updated?.enabled) this.#schedule(updated);
  }

  async #markSkipped(task: ScheduledTask): Promise<void> {
    const updated = await this.#store.updateScheduledTask(task.task_id, (current) => {
      current.skipped_runs += 1;
      current.next_run_at = current.enabled
        ? nextRun(current.schedule, new Date()).toISOString()
        : null;
      current.updated_at = new Date().toISOString();
      current.revision += 1;
    });
    if (updated?.enabled) this.#schedule(updated);
  }

  #cancelTimer(taskId: string): void {
    const timer = this.#timers.get(taskId);
    if (timer !== undefined) clearTimeout(timer);
    this.#timers.delete(taskId);
  }

  async #createTask(task: ScheduledTask): Promise<void> {
    const previous = this.#creationTail;
    let release: (() => void) | undefined;
    this.#creationTail = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      if ((await this.#store.listScheduledTasks()).length >= MAX_SCHEDULED_TASKS) {
        throw new ApiError(
          429,
          "scheduled_task_capacity",
          `scheduled-task limit reached (capacity ${MAX_SCHEDULED_TASKS})`,
        );
      }
      await this.#store.putScheduledTask(task);
    } finally {
      release?.();
    }
  }
}

function waitForRun(
  actor: import("./session-actor.js").SessionActor,
  runId: string,
): Promise<{ outcome: "succeeded" | "failed" | "stopped"; error: string | null }> {
  return new Promise((resolve) => {
    const unsubscribe = actor.subscribe((envelope) => {
      if (envelope.run_id !== runId) return;
      if (envelope.event.type === "run_completed") {
        unsubscribe();
        resolve({ outcome: "succeeded", error: null });
      } else if (envelope.event.type === "run_stopped") {
        unsubscribe();
        resolve({ outcome: "stopped", error: null });
      } else if (envelope.event.type === "run_failed") {
        unsubscribe();
        resolve({ outcome: "failed", error: envelope.event.message });
      }
    });
  });
}

export function nextRun(schedule: ScheduledTaskSchedule, after: Date): Date {
  if (schedule.type === "interval") {
    const multiplier = schedule.unit === "minutes" ? 60_000 : schedule.unit === "hours" ? 3_600_000 : 86_400_000;
    return new Date(after.getTime() + schedule.every * multiplier);
  }
  const [hourText, minuteText] = schedule.time.split(":");
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const weekdays = new Set(schedule.weekdays.map((day) => day.toLowerCase()));
  const formatter = new Intl.DateTimeFormat("en-US", {
    timeZone: schedule.timezone,
    weekday: "long",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  let candidate = new Date(Math.floor(after.getTime() / 60_000) * 60_000 + 60_000);
  for (let index = 0; index < 8 * 24 * 60; index += 1) {
    const parts = Object.fromEntries(
      formatter
        .formatToParts(candidate)
        .filter((part) => part.type !== "literal")
        .map((part) => [part.type, part.value]),
    );
    if (
      Number(parts.hour) === hour &&
      Number(parts.minute) === minute &&
      weekdays.has((parts.weekday ?? "").toLowerCase())
    ) {
      return candidate;
    }
    candidate = new Date(candidate.getTime() + 60_000);
  }
  throw new Error("could not compute the next daily schedule occurrence");
}

function advancePast(
  schedule: ScheduledTaskSchedule,
  scheduledFor: Date,
  now: Date,
): Date {
  if (schedule.type === "daily") return nextRun(schedule, now);
  const multiplier =
    schedule.unit === "minutes" ? 60_000 : schedule.unit === "hours" ? 3_600_000 : 86_400_000;
  const interval = schedule.every * multiplier;
  const elapsed = Math.max(0, now.getTime() - scheduledFor.getTime());
  const steps = Math.floor(elapsed / interval) + 1;
  return new Date(scheduledFor.getTime() + steps * interval);
}

function normalizeCreateRequest(
  request: CreateScheduledTaskRequest,
  defaultWorkspace: string,
): {
  name: string;
  prompt: string;
  workspace: string;
  profile_id: string;
  agent_profile_id: string;
  capability_mode: CapabilityMode | null;
  schedule: ScheduledTaskSchedule;
} {
  if (typeof request.name !== "string" || !request.name.trim()) {
    throw invalid("name must not be empty");
  }
  const name = request.name.trim();
  if ([...name].length > MAX_NAME_CHARS) throw invalid(`name must not exceed ${MAX_NAME_CHARS} characters`);
  if (/\p{Cc}/u.test(name)) throw invalid("name must not contain control characters");
  if (typeof request.prompt !== "string" || !request.prompt.trim()) {
    throw invalid("prompt must not be empty");
  }
  const prompt = request.prompt.trim();
  if ([...prompt].length > MAX_PROMPT_CHARS) {
    throw invalid(`prompt must not exceed ${MAX_PROMPT_CHARS} characters`);
  }
  if (request.workspace !== undefined && request.workspace !== null && typeof request.workspace !== "string") {
    throw invalid("workspace must be a string");
  }
  if (request.profile_id !== undefined && request.profile_id !== null && typeof request.profile_id !== "string") {
    throw invalid("profile_id must be a string");
  }
  if (request.agent_profile_id !== undefined && request.agent_profile_id !== null && typeof request.agent_profile_id !== "string") {
    throw invalid("agent_profile_id must be a string");
  }
  if (
    request.capability_mode !== undefined &&
    request.capability_mode !== null &&
    !isCapabilityMode(request.capability_mode)
  ) {
    throw invalid("capability_mode is invalid");
  }
  const profileId = normalizeIdentifier("profile_id", request.profile_id ?? "default");
  const agentProfileId = normalizeIdentifier(
    "agent_profile_id",
    request.agent_profile_id ?? "default",
  );
  return {
    name,
    prompt,
    workspace: request.workspace ?? defaultWorkspace,
    profile_id: profileId,
    agent_profile_id: agentProfileId,
    capability_mode: request.capability_mode ?? null,
    schedule: normalizeSchedule(request.schedule),
  };
}

function normalizeSchedule(schedule: ScheduledTaskSchedule): ScheduledTaskSchedule {
  if (!isRecord(schedule) || (schedule.type !== "interval" && schedule.type !== "daily")) {
    throw invalid("schedule is invalid");
  }
  if (schedule.type === "interval") {
    assertScheduleKeys(schedule, ["type", "every", "unit"]);
    if (!Number.isSafeInteger(schedule.every) || schedule.every <= 0) {
      throw invalid("interval every must be a positive integer");
    }
    if (!["minutes", "hours", "days"].includes(schedule.unit)) {
      throw invalid("interval unit is invalid");
    }
    const unitSeconds = schedule.unit === "minutes" ? 60 : schedule.unit === "hours" ? 3_600 : 86_400;
    if (schedule.every * unitSeconds > MAX_INTERVAL_SECONDS) {
      throw invalid("interval must not exceed ten years");
    }
    return { type: "interval", every: schedule.every, unit: schedule.unit };
  }
  assertScheduleKeys(schedule, ["type", "time", "weekdays", "timezone"]);
  if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(schedule.time)) throw invalid("daily time must be HH:MM");
  if (!Array.isArray(schedule.weekdays) || schedule.weekdays.length === 0) {
    throw invalid("daily weekdays must not be empty");
  }
  const validWeekdays = new Set([
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
  ]);
  if (
    schedule.weekdays.some((weekday) =>
      typeof weekday !== "string" || !validWeekdays.has(weekday.toLowerCase()),
    )
  ) {
    throw invalid("daily weekdays contain an invalid value");
  }
  if (typeof schedule.timezone !== "string" || !schedule.timezone) {
    throw invalid("daily timezone must not be empty");
  }
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: schedule.timezone }).format();
  } catch {
    throw invalid("daily timezone is invalid");
  }
  const order = [...validWeekdays];
  const weekdays = [...new Set(schedule.weekdays.map((weekday) => weekday.toLowerCase()))]
    .sort((left, right) => order.indexOf(left) - order.indexOf(right)) as ScheduledWeekday[];
  return {
    type: "daily",
    time: schedule.time,
    weekdays,
    timezone: schedule.timezone,
  };
}

function normalizeIdentifier(field: string, value: string): string {
  const normalized = value.trim();
  if (!normalized) throw invalid(`${field} must not be empty`);
  if (Buffer.byteLength(normalized) > 128) throw invalid(`${field} must not exceed 128 bytes`);
  if (/\p{Cc}/u.test(normalized)) throw invalid(`${field} must not contain control characters`);
  return normalized;
}

function assertScheduleKeys(
  schedule: Record<string, unknown>,
  allowed: readonly string[],
): void {
  const allowedKeys = new Set(allowed);
  const unknown = Object.keys(schedule).find((key) => !allowedKeys.has(key));
  if (unknown !== undefined) throw invalid(`schedule contains unknown field \`${unknown}\``);
}

function invalid(message: string): ApiError {
  return new ApiError(400, "invalid_scheduled_task", message);
}

function notFound(taskId: string): ApiError {
  return new ApiError(404, "scheduled_task_not_found", `scheduled task \`${taskId}\` was not found`);
}

export function capabilityForTask(task: ScheduledTask): CapabilityMode | undefined {
  return task.capability_mode ?? undefined;
}
