/**
 * TypeScript mirror of the public phi-daemon v1 wire protocol.
 *
 * Keep protocol objects provider-neutral. Pi-specific messages and events are
 * converted at the runtime boundary in projection.ts.
 */

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export type Role = "system" | "user" | "assistant" | "tool";
export type MessageVisibility = "public" | "internal";
export type ImageDetail = "auto" | "low" | "high";

export interface ImageUrl {
  url: string;
  detail?: ImageDetail | null;
}

export interface DocumentPart {
  filename: string;
  mime_type: string;
  data: string;
}

export type ContentPart =
  | { type: "text"; text: string }
  | { type: "image_url"; image_url: ImageUrl }
  | { type: "document"; document: DocumentPart };

export type Content =
  | { type: "text"; value: string }
  | { type: "parts"; value: ContentPart[] };

export interface ToolCall {
  id: string;
  name: string;
  arguments: JsonValue;
}

export interface PublicMessage {
  role: Role;
  visibility?: MessageVisibility;
  content: Content | null;
  reasoning?: string | null;
  tool_calls: ToolCall[];
  tool_call_id: string | null;
  tool_result_is_error: boolean;
  tool_result_metadata?: JsonValue;
}

export type ReasoningEffort =
  | "none"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export type CapabilityMode = "read_only" | "workspace_edit" | "full_access";
export type ToolEffect =
  | "read_only"
  | "internal"
  | "workspace_write"
  | "external_side_effect";

export type SessionStatus =
  | "awaiting_first_prompt"
  | "idle"
  | "compacting"
  | "running"
  | "stopping"
  | "closing"
  | "closed"
  | "offline";

export interface SessionConfig {
  model: string;
  reasoning_effort: ReasoningEffort | null;
  revision: number;
}

export interface AgentProfileRef {
  agent_profile_id: string;
  revision: number;
}

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cached_input_tokens: number;
}

export interface ContextUsage {
  max_tokens: number;
  used_tokens: number;
  remaining_tokens: number;
}

export interface Usage {
  last: TokenUsage | null;
  context: ContextUsage | null;
  cumulative: TokenUsage;
}

export interface ToolCallDraft {
  index: number;
  id: string | null;
  name: string | null;
  arguments: string;
}

export interface AssistantDraft {
  reasoning: string;
  text: string;
  tool_calls: ToolCallDraft[];
  fork_message_index?: number;
}

export interface AskUserOption {
  label: string;
  description: string;
  preview?: string;
}

export interface AskUserQuestion {
  question: string;
  header: string;
  options: AskUserOption[];
  multiSelect: boolean;
}

export interface AskUserRequest {
  ask_id: string;
  questions: AskUserQuestion[];
}

export interface AskUserAnswer {
  question_index: number;
  selected_options: string[];
  custom_text: string | null;
}

export interface ToolPermissionRule {
  tool_name: string;
  pattern?: string;
}

export interface ToolPermissionPrompt {
  permission_id: string;
  call: ToolCall;
  effect: ToolEffect;
  capability_mode: CapabilityMode;
  suggestions: ToolPermissionRule[];
}

export type ToolPermissionDecision =
  | { type: "allow_once" }
  | { type: "allow_for_session"; rule: ToolPermissionRule }
  | { type: "deny"; message?: string };

export interface ContextCompactionStatus {
  phase: "started" | "completed" | "failed";
  history_index: number;
  after_message_count?: number;
  message?: string;
}

export interface SkillSummary {
  name: string;
  display_name?: string | null;
  description: string;
  when_to_use?: string | null;
  argument_hint?: string | null;
  arguments?: string[];
  version?: string | null;
  model_invocable: boolean;
  user_invocable: boolean;
  source?: string | null;
}

export interface SkillDiagnostic {
  level: "warning" | "error";
  code: string;
  message: string;
}

export interface SubagentSummary {
  agent_id: string;
  description: string;
  state: "starting" | "running" | "idle" | "closing" | "closed";
  last_sequence: number;
  observer_path: string;
}

export interface SessionDto {
  session_id: string;
  title: string | null;
  profile_id: string;
  agent_profile: AgentProfileRef;
  workspace?: string | null;
  initialized: boolean;
  status: SessionStatus;
  active_run_id: string | null;
  queued_runs: number;
  capability_mode: CapabilityMode;
  config: SessionConfig;
  history: PublicMessage[];
  context_compactions?: ContextCompactionStatus[];
  context_compaction?: ContextCompactionStatus;
  draft: AssistantDraft | null;
  pending_asks: AskUserRequest[];
  pending_tool_permissions?: ToolPermissionPrompt[];
  skills?: readonly SkillSummary[];
  subagents: SubagentSummary[];
  usage: Usage;
  last_sequence: number;
}

export interface ToolProgress {
  message: string;
  metadata?: JsonValue;
}

export type ContextCompactionTrigger =
  | { type: "automatic"; usage: ContextUsage }
  | { type: "manual"; instructions: string | null }
  | { type: "context_length_exceeded" };

export type RetryReason =
  | { type: "request_timeout"; timeout_ms: number }
  | { type: "transport"; message: string }
  | { type: "http_status"; status: number; body: string };

export type AssistantDelta =
  | { type: "reasoning"; delta: string }
  | { type: "text"; delta: string }
  | {
      type: "tool_call";
      index: number;
      id: string | null;
      name: string | null;
      arguments_delta: string;
    };

export type EventDto =
  | { type: "state_changed"; status: SessionStatus }
  | { type: "session_initialized" }
  | { type: "title_changed"; title: string }
  | { type: "run_queued"; run_id: string }
  | { type: "run_started"; run_id: string }
  | { type: "run_completed"; run_id: string }
  | { type: "run_stopped"; run_id: string }
  | { type: "run_failed"; run_id: string; message: string }
  | { type: "config_changed"; config: SessionConfig }
  | { type: "capability_mode_changed"; capability_mode: CapabilityMode }
  | { type: "askuser_requested"; request: AskUserRequest }
  | { type: "askuser_answered"; ask_id: string }
  | { type: "askuser_cancelled"; ask_id: string }
  | { type: "tool_permission_requested"; request: ToolPermissionPrompt }
  | { type: "tool_permission_resolved"; permission_id: string; allowed: boolean }
  | { type: "tool_permission_cancelled"; permission_id: string }
  | { type: "operation_failed"; operation: string; message: string }
  | { type: "actor_crashed"; message: string }
  | { type: "agent_start" }
  | { type: "agent_end" }
  | { type: "agent_stopped" }
  | { type: "turn_start"; turn: number }
  | {
      type: "turn_end";
      turn: number;
      message: PublicMessage;
      tool_results: PublicMessage[];
    }
  | { type: "message_start"; message: PublicMessage }
  | { type: "message_update"; delta: AssistantDelta }
  | { type: "message_end"; message: PublicMessage }
  | { type: "message_aborted" }
  | { type: "tool_execution_start"; call: ToolCall }
  | { type: "tool_execution_progress"; call: ToolCall; progress: ToolProgress }
  | {
      type: "tool_execution_end";
      call: ToolCall;
      content: string;
      is_error: boolean;
      content_parts?: ContentPart[];
      metadata?: JsonValue;
    }
  | { type: "usage_update"; usage: TokenUsage; context_usage: ContextUsage | null }
  | {
      type: "provider_retry";
      retry_number: number;
      max_retries: number;
      delay_ms: number;
      reason: RetryReason;
    }
  | {
      type: "context_compaction_started";
      trigger: ContextCompactionTrigger;
      compactor: string;
    }
  | {
      type: "context_compaction_completed";
      trigger: ContextCompactionTrigger;
      compactor: string;
      before_message_count: number;
      after_message_count: number;
      usage: TokenUsage | null;
      estimated_context_tokens: number;
    }
  | {
      type: "context_compaction_failed";
      trigger: ContextCompactionTrigger;
      compactor: string;
      message: string;
    }
  | { type: "error"; message: string };

export interface EventEnvelope {
  type: "event";
  sequence: number;
  session_id: string;
  run_id?: string;
  event: EventDto;
}

export type ServerMessage =
  | { type: "building" }
  | {
      type: "ready";
      config: SessionConfig;
      capability_mode: CapabilityMode;
      agent_profile: AgentProfileRef;
      workspace?: string | null;
      skills?: readonly SkillSummary[];
    }
  | { type: "session_created"; session_id: string }
  | { type: "snapshot"; session: SessionDto }
  | {
      type: "command_accepted";
      request_id: string;
      command: string;
      run_id?: string;
      queue_position?: number;
    }
  | { type: "command_rejected"; request_id: string; code: string; message: string }
  | EventEnvelope
  | { type: "resync_required"; skipped: number; session: SessionDto }
  | { type: "pong"; request_id: string }
  | { type: "fatal_error"; code: string; message: string };

export interface SkillInvocation {
  name: string;
  arguments?: string | null;
}

export type ClientCommand =
  | { type: "prompt"; request_id: string; content: Content; skill?: SkillInvocation }
  | { type: "stop"; request_id: string; run_id: string }
  | { type: "compact"; request_id: string; instructions?: string | null }
  | { type: "set_model"; request_id: string; model: string }
  | {
      type: "set_reasoning_effort";
      request_id: string;
      effort: ReasoningEffort | null;
    }
  | {
      type: "set_capability_mode";
      request_id: string;
      capability_mode: CapabilityMode;
    }
  | {
      type: "answer_askuser";
      request_id: string;
      ask_id: string;
      answers: AskUserAnswer[];
    }
  | {
      type: "decide_tool_permission";
      request_id: string;
      permission_id: string;
      decision: ToolPermissionDecision;
    }
  | { type: "ping"; request_id: string };

export type ProviderKind = "openai_chat" | "openai_responses" | "anthropic";

export interface PutProviderRequest {
  provider: ProviderKind;
  api_key: string;
  base_url: string;
  model: string;
  /** Accepted for phi-daemon compatibility; coding-agent profiles own the prompt. */
  system_prompt?: string | null;
  max_output_tokens?: number | null;
  max_context_tokens: number;
  temperature?: number | null;
  reasoning_effort?: ReasoningEffort | null;
  max_retries?: number;
  request_timeout_secs?: number;
  stream_idle_timeout_secs?: number;
}

export interface PublicProviderConfig {
  profile_id: string;
  provider: ProviderKind;
  api_key_configured: boolean;
  base_url: string;
  model: string;
  system_prompt: string | null;
  max_output_tokens: number | null;
  max_context_tokens: number;
  temperature: number | null;
  reasoning_effort: ReasoningEffort | null;
  max_retries: number;
  request_timeout_secs: number;
  stream_idle_timeout_secs: number;
  revision: number;
}

export interface ProviderResponse {
  configured: boolean;
  provider: PublicProviderConfig | null;
}

export interface ProvidersResponse {
  providers: PublicProviderConfig[];
}

export type PromptMode = "extend" | "full";

export interface PromptDefinition {
  mode?: PromptMode;
  text?: string;
}

export interface NamePolicy {
  allow?: string[] | null;
  deny?: string[];
}

export interface PutAgentProfileRequest {
  prompt?: PromptDefinition;
  tools?: NamePolicy;
  skills?: NamePolicy;
  /** Deprecated phi wire field; accepted and ignored. */
  initial_agent_mode?: "default" | "plan" | null;
  initial_capability_mode?: CapabilityMode;
  model?: string | null;
  reasoning_effort?: ReasoningEffort | null;
}

export interface PublicAgentProfile {
  agent_profile_id: string;
  revision: number;
  prompt: { mode: PromptMode; text: string };
  tools: { allow: string[] | null; deny: string[] };
  skills: { allow: string[] | null; deny: string[] };
  initial_capability_mode: CapabilityMode;
  model: string | null;
  reasoning_effort: ReasoningEffort | null;
}

export interface AgentProfileResponse {
  configured: boolean;
  agent_profile: PublicAgentProfile | null;
}

export interface AgentProfilesResponse {
  agent_profiles: PublicAgentProfile[];
}

export interface SessionSummary {
  session_id: string;
  title: string | null;
  pinned: boolean;
  profile_id: string;
  agent_profile: AgentProfileRef;
  workspace?: string | null;
  status: SessionStatus;
  active_run_id: string | null;
  queued_runs: number;
  capability_mode: CapabilityMode | null;
  config: SessionConfig;
  message_count: number | null;
  subagents: SubagentSummary[];
}

export type ForkPosition = "after" | "before_tool_calls";

export interface WorkspaceSessionGroup {
  workspace: string | null;
  sessions: SessionSummary[];
}

export interface SessionsResponse {
  sessions: SessionSummary[];
  workspaces: WorkspaceSessionGroup[];
}

export interface SkillsResponse {
  session_id: string;
  skills: SkillSummary[];
  diagnostics?: SkillDiagnostic[];
}

export interface WorkspaceDirectory {
  name: string;
  path: string;
}

export interface WorkspaceBrowseResponse {
  path: string;
  parent: string | null;
  directories: WorkspaceDirectory[];
  truncated: boolean;
}

export type ScheduledWeekday =
  | "monday"
  | "tuesday"
  | "wednesday"
  | "thursday"
  | "friday"
  | "saturday"
  | "sunday";

export type ScheduledIntervalUnit = "minutes" | "hours" | "days";

export type ScheduledTaskSchedule =
  | { type: "daily"; time: string; weekdays: ScheduledWeekday[]; timezone: string }
  | { type: "interval"; every: number; unit: ScheduledIntervalUnit };

export type ScheduledRunOutcome =
  | "running"
  | "succeeded"
  | "failed"
  | "stopped"
  | "interrupted";

export interface ScheduledTaskRun {
  scheduled_for: string;
  started_at: string;
  finished_at: string | null;
  outcome: ScheduledRunOutcome;
  session_id: string | null;
  error: string | null;
}

export interface ScheduledTask {
  task_id: string;
  name: string;
  prompt: string;
  workspace: string;
  profile_id: string;
  agent_profile_id: string;
  capability_mode: CapabilityMode | null;
  schedule: ScheduledTaskSchedule;
  enabled: boolean;
  created_at: string;
  updated_at: string;
  next_run_at: string | null;
  last_run: ScheduledTaskRun | null;
  skipped_runs: number;
  revision: number;
}

export interface ScheduledTasksResponse {
  tasks: ScheduledTask[];
}

export interface CreateScheduledTaskRequest {
  name: string;
  prompt: string;
  workspace?: string | null;
  profile_id?: string | null;
  agent_profile_id?: string | null;
  capability_mode?: CapabilityMode | null;
  schedule: ScheduledTaskSchedule;
}

export interface UpdateScheduledTaskRequest {
  enabled: boolean;
  expected_revision?: number;
}

export interface AuthTokenResponse {
  token: string;
  token_type: "websocket_subprotocol";
  protocol: "phi.v1";
  expires_in_secs: number;
}

export interface ErrorResponse {
  code: string;
  message: string;
}

const CAPABILITY_MODES = new Set<CapabilityMode>([
  "read_only",
  "workspace_edit",
  "full_access",
]);
const REASONING_EFFORTS = new Set<ReasoningEffort>([
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
]);

export function isCapabilityMode(value: unknown): value is CapabilityMode {
  return typeof value === "string" && CAPABILITY_MODES.has(value as CapabilityMode);
}

export function isReasoningEffort(value: unknown): value is ReasoningEffort {
  return typeof value === "string" && REASONING_EFFORTS.has(value as ReasoningEffort);
}

export function isContent(value: unknown): value is Content {
  if (!isRecord(value)) return false;
  if (value.type === "text") return typeof value.value === "string";
  if (value.type !== "parts" || !Array.isArray(value.value)) return false;
  return value.value.every((part) => {
    if (!isRecord(part) || typeof part.type !== "string") return false;
    if (part.type === "text") {
      return typeof part.text === "string";
    }
    if (part.type === "image_url") {
      return (
        isRecord(part.image_url) &&
        typeof part.image_url.url === "string" &&
        (part.image_url.detail === undefined ||
          part.image_url.detail === null ||
          ["auto", "low", "high"].includes(part.image_url.detail as string))
      );
    }
    if (part.type === "document") {
      return (
        isRecord(part.document) &&
        typeof part.document.filename === "string" &&
        typeof part.document.mime_type === "string" &&
        typeof part.document.data === "string"
      );
    }
    return false;
  });
}

export function parseClientCommand(value: unknown): ClientCommand {
  if (!isRecord(value) || typeof value.type !== "string") {
    throw new Error("command must be an object with a string `type`");
  }
  if (typeof value.request_id !== "string") {
    throw new Error("command `request_id` must be a string");
  }
  switch (value.type) {
    case "prompt": {
      if (!isContent(value.content)) throw new Error("prompt `content` is invalid");
      if (value.skill !== undefined) {
        if (!isRecord(value.skill) || typeof value.skill.name !== "string") {
          throw new Error("prompt `skill` is invalid");
        }
        if (
          value.skill.arguments !== undefined &&
          value.skill.arguments !== null &&
          typeof value.skill.arguments !== "string"
        ) {
          throw new Error("prompt skill `arguments` must be a string or null");
        }
      }
      return value as unknown as ClientCommand;
    }
    case "stop":
      if (typeof value.run_id !== "string") throw new Error("stop `run_id` must be a string");
      return value as unknown as ClientCommand;
    case "compact":
      if (
        value.instructions !== undefined &&
        value.instructions !== null &&
        typeof value.instructions !== "string"
      ) {
        throw new Error("compact `instructions` must be a string or null");
      }
      return value as unknown as ClientCommand;
    case "set_model":
      if (typeof value.model !== "string") throw new Error("set_model `model` must be a string");
      return value as unknown as ClientCommand;
    case "set_reasoning_effort": {
      const effort = value.effort ?? null;
      if (effort !== null && !isReasoningEffort(effort)) {
        throw new Error("set_reasoning_effort `effort` is invalid");
      }
      return { ...value, effort } as ClientCommand;
    }
    case "set_capability_mode":
      if (!isCapabilityMode(value.capability_mode)) {
        throw new Error("set_capability_mode `capability_mode` is invalid");
      }
      return value as unknown as ClientCommand;
    case "answer_askuser":
      if (typeof value.ask_id !== "string" || !Array.isArray(value.answers)) {
        throw new Error("answer_askuser payload is invalid");
      }
      const answers = value.answers.map(normalizeAskUserAnswer);
      if (answers.some((answer) => answer === undefined)) {
        throw new Error("answer_askuser answers are invalid");
      }
      return { ...value, answers } as ClientCommand;
    case "decide_tool_permission": {
      if (typeof value.permission_id !== "string" || !isRecord(value.decision)) {
        throw new Error("decide_tool_permission payload is invalid");
      }
      const decision = normalizeToolPermissionDecision(value.decision);
      if (decision === undefined) {
        throw new Error("decide_tool_permission decision is invalid");
      }
      return { ...value, decision } as ClientCommand;
    }
    case "ping":
      return value as unknown as ClientCommand;
    default:
      throw new Error(`unknown command type: ${value.type}`);
  }
}

function normalizeAskUserAnswer(value: unknown): AskUserAnswer | undefined {
  if (!isRecord(value)) return undefined;
  if (!Number.isSafeInteger(value.question_index) || (value.question_index as number) < 0) {
    return undefined;
  }
  const selected = value.selected_options ?? [];
  const customText = value.custom_text ?? null;
  if (
    !Array.isArray(selected) ||
    !selected.every((option) => typeof option === "string") ||
    (customText !== null && typeof customText !== "string")
  ) {
    return undefined;
  }
  return {
    question_index: value.question_index as number,
    selected_options: selected,
    custom_text: customText,
  };
}

function normalizeToolPermissionDecision(
  value: Record<string, unknown>,
): ToolPermissionDecision | undefined {
  if (value.type === "allow_once") {
    return Object.keys(value).length === 1 ? { type: "allow_once" } : undefined;
  }
  if (value.type === "deny") {
    if (!Object.keys(value).every((key) => key === "type" || key === "message")) return undefined;
    if (value.message !== undefined && value.message !== null && typeof value.message !== "string") {
      return undefined;
    }
    return value.message === undefined || value.message === null
      ? { type: "deny" }
      : { type: "deny", message: value.message };
  }
  if (value.type !== "allow_for_session" || !isRecord(value.rule)) return undefined;
  if (
    typeof value.rule.tool_name !== "string" ||
    (value.rule.pattern !== undefined &&
      value.rule.pattern !== null &&
      typeof value.rule.pattern !== "string") ||
    !Object.keys(value.rule).every((key) => key === "tool_name" || key === "pattern")
  ) {
    return undefined;
  }
  return {
    type: "allow_for_session",
    rule: {
      tool_name: value.rule.tool_name,
      ...(typeof value.rule.pattern === "string" ? { pattern: value.rule.pattern } : {}),
    },
  };
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function toJsonValue(value: unknown): JsonValue {
  if (value === undefined) return null;
  try {
    const serialized = JSON.stringify(value);
    return serialized === undefined ? null : (JSON.parse(serialized) as JsonValue);
  } catch {
    return null;
  }
}
