/**
 * TypeScript mirror of the phi-daemon wire protocol.
 *
 * These types are derived from the public DTOs in
 * `crates/phi-daemon/src/api/dto.rs` and the library content/tool types in
 * `src/types.rs`. They describe the JSON shapes that travel over HTTP and the
 * WebSocket application frames. Discriminated unions use a literal `type`
 * field that matches the daemon's `#[serde(tag = "type", rename_all =
 * "snake_case")]`.
 */

/* -------------------------------------------------------------------------- */
/* Shared content / message types                                             */
/* -------------------------------------------------------------------------- */

export type Role = 'system' | 'user' | 'assistant' | 'tool';

export interface ImageUrl {
  url: string;
  detail?: ImageDetail | null;
}

export type ImageDetail = 'auto' | 'low' | 'high';

export interface DocumentPart {
  filename: string;
  mime_type: string;
  data: string;
}

/** `#[serde(tag = "type", rename_all = "snake_case")]` */
export type ContentPart =
  | { type: 'text'; text: string }
  | { type: 'image_url'; image_url: ImageUrl }
  | { type: 'document'; document: DocumentPart };

/** `#[serde(tag = "type", content = "value", rename_all = "snake_case")]` */
export type Content =
  | { type: 'text'; value: string }
  | { type: 'parts'; value: ContentPart[] };

export interface ToolCall {
  id: string;
  name: string;
  /** Arbitrary JSON arguments. */
  arguments: unknown;
}

/** A provider-neutral message as projected in the public session history. */
export interface PublicMessage {
  role: Role;
  content: Content | null;
  tool_calls: ToolCall[];
  tool_call_id: string | null;
  tool_result_is_error: boolean;
  tool_result_metadata?: unknown;
}

/* -------------------------------------------------------------------------- */
/* Reasoning / mode / status                                                  */
/* -------------------------------------------------------------------------- */

export type ReasoningEffort =
  | 'none'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'xhigh'
  | 'max';

export type AgentMode = 'default' | 'plan';

export type SessionStatus =
  | 'awaiting_first_prompt'
  | 'idle'
  | 'compacting'
  | 'running'
  | 'stopping'
  | 'closing'
  | 'closed'
  | 'offline';

export interface SessionConfig {
  model: string;
  reasoning_effort: ReasoningEffort | null;
  revision: number;
}

/* -------------------------------------------------------------------------- */
/* Token usage                                                                */
/* -------------------------------------------------------------------------- */

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

/* -------------------------------------------------------------------------- */
/* Drafts                                                                     */
/* -------------------------------------------------------------------------- */

export interface ToolCallDraft {
  index: number;
  id: string | null;
  name: string | null;
  arguments: string;
}

export interface AssistantDraft {
  text: string;
  tool_calls: ToolCallDraft[];
}

/* -------------------------------------------------------------------------- */
/* Subagents (parent projection)                                             */
/* -------------------------------------------------------------------------- */

export type SubagentState =
  | 'starting'
  | 'running'
  | 'idle'
  | 'closing'
  | 'closed';

export interface SubagentSummary {
  agent_id: string;
  description: string;
  state: SubagentState;
  last_sequence: number;
  observer_path: string;
}

/* -------------------------------------------------------------------------- */
/* askuser                                                                    */
/* -------------------------------------------------------------------------- */

export interface AskUserOption {
  label: string;
  description: string;
  preview?: string;
}

export interface AskUserQuestion {
  question: string;
  header: string;
  options: AskUserOption[];
  multi_select: boolean;
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

/* -------------------------------------------------------------------------- */
/* Plan mode                                                                  */
/* -------------------------------------------------------------------------- */

export interface PlanArtifact {
  session_id: string;
  revision: number;
  content: string;
}

export interface PlanApprovalRequest {
  approval_id: string;
  plan: PlanArtifact;
}

export type PlanApprovalDecision =
  | { type: 'approve'; revision: number }
  | { type: 'reject'; revision: number; feedback?: string | null };

/* -------------------------------------------------------------------------- */
/* Session snapshot DTO                                                       */
/* -------------------------------------------------------------------------- */

export interface SessionDto {
  session_id: string;
  profile_id: string;
  initialized: boolean;
  status: SessionStatus;
  active_run_id: string | null;
  queued_runs: number;
  mode: AgentMode;
  config: SessionConfig;
  history: PublicMessage[];
  draft: AssistantDraft | null;
  pending_asks: AskUserRequest[];
  pending_plan_approvals: PlanApprovalRequest[];
  subagents: SubagentSummary[];
  usage: Usage;
  last_sequence: number;
}

/* -------------------------------------------------------------------------- */
/* Tool progress                                                              */
/* -------------------------------------------------------------------------- */

export interface ToolProgress {
  message: string;
  metadata?: unknown;
}

/* -------------------------------------------------------------------------- */
/* Event payloads (EventDto)                                                  */
/* -------------------------------------------------------------------------- */

export interface CompactionTrigger {
  type: 'automatic';
  usage: ContextUsage;
}
export interface ManualCompactionTrigger {
  type: 'manual';
  instructions: string | null;
}
export interface ContextLengthTrigger {
  type: 'context_length_exceeded';
}
export type ContextCompactionTrigger =
  | CompactionTrigger
  | ManualCompactionTrigger
  | ContextLengthTrigger;

export type RetryReason =
  | { type: 'request_timeout'; timeout_ms: number }
  | { type: 'transport'; message: string }
  | { type: 'http_status'; status: number; body: string };

/** A streaming delta from `message_update`. */
export type AssistantDelta =
  | { type: 'text'; delta: string }
  | {
      type: 'tool_call';
      index: number;
      id: string | null;
      name: string | null;
      arguments_delta: string;
    };

/**
 * The full set of runtime/agent event payloads.
 * Discriminated on `type`, snake_case.
 */
export type EventDto =
  | { type: 'state_changed'; status: SessionStatus }
  | { type: 'session_initialized' }
  | { type: 'run_queued'; run_id: string }
  | { type: 'run_started'; run_id: string }
  | { type: 'run_completed'; run_id: string }
  | { type: 'run_stopped'; run_id: string }
  | { type: 'run_failed'; run_id: string; message: string }
  | { type: 'config_changed'; config: SessionConfig }
  | { type: 'mode_changed'; mode: AgentMode }
  | { type: 'askuser_requested'; request: AskUserRequest }
  | { type: 'askuser_answered'; ask_id: string }
  | { type: 'askuser_cancelled'; ask_id: string }
  | { type: 'plan_approval_requested'; request: PlanApprovalRequest }
  | {
      type: 'plan_approval_decided';
      approval_id: string;
      decision: PlanApprovalDecision;
    }
  | { type: 'plan_approval_cancelled'; approval_id: string }
  | { type: 'operation_failed'; operation: string; message: string }
  | { type: 'actor_crashed'; message: string }
  | {
      type: 'subagent_spawned';
      agent_id: string;
      description: string;
      initial_delivery_id: string;
      observer_path: string;
    }
  | { type: 'subagent_state_changed'; agent_id: string; state: SubagentState }
  | { type: 'subagent_message_queued'; agent_id: string; delivery_id: string }
  | {
      type: 'subagent_notification';
      agent_id: string;
      notification: SubagentNotification;
    }
  | { type: 'subagent_agent_event'; agent_id: string; event: EventDto }
  | {
      type: 'subagent_run_finished';
      agent_id: string;
      run_id: string;
      outcome: unknown;
    }
  | {
      type: 'subagent_closed';
      agent_id: string;
      delivery_id: string;
      reason: string;
      wake_parent: boolean;
    }
  | { type: 'subagents_resynced'; subagents: SubagentSummary[] }
  | { type: 'agent_start' }
  | { type: 'agent_end' }
  | { type: 'agent_stopped' }
  | { type: 'turn_start'; turn: number }
  | {
      type: 'turn_end';
      turn: number;
      message: PublicMessage;
      tool_results: PublicMessage[];
    }
  | { type: 'message_start'; message: PublicMessage }
  | { type: 'message_update'; delta: AssistantDelta }
  | { type: 'message_end'; message: PublicMessage }
  | { type: 'message_aborted' }
  | { type: 'tool_execution_start'; call: ToolCall }
  | { type: 'tool_execution_progress'; call: ToolCall; progress: ToolProgress }
  | {
      type: 'tool_execution_end';
      call: ToolCall;
      content: string;
      is_error: boolean;
      content_parts?: ContentPart[];
      metadata?: unknown;
    }
  | {
      type: 'usage_update';
      usage: TokenUsage;
      context_usage: ContextUsage | null;
    }
  | {
      type: 'provider_retry';
      retry_number: number;
      max_retries: number;
      delay_ms: number;
      reason: RetryReason;
    }
  | {
      type: 'context_compaction_started';
      trigger: ContextCompactionTrigger;
      compactor: string;
      prompt: string;
    }
  | {
      type: 'context_compaction_completed';
      trigger: ContextCompactionTrigger;
      compactor: string;
      before_message_count: number;
      after_message_count: number;
      changed_from: number;
      replacement: PublicMessage[];
      summary: string;
      usage: TokenUsage | null;
      estimated_context_tokens: number;
    }
  | {
      type: 'context_compaction_failed';
      trigger: ContextCompactionTrigger;
      compactor: string;
      message: string;
    }
  | { type: 'error'; message: string };

export interface SubagentNotification {
  delivery_id: string;
  kind: 'progress' | 'blocker' | 'result' | 'failed' | 'closed';
  source: 'child' | 'runtime';
  message: string;
  wake_parent: boolean;
}

/* -------------------------------------------------------------------------- */
/* Server frames (top-level WebSocket messages)                              */
/* -------------------------------------------------------------------------- */

export interface EventEnvelope {
  type: 'event';
  sequence: number;
  session_id: string;
  run_id?: string;
  event: EventDto;
}

export type ServerMessage =
  | { type: 'building' }
  | { type: 'ready'; config: SessionConfig; mode: AgentMode }
  | { type: 'session_created'; session_id: string }
  | { type: 'snapshot'; session: SessionDto }
  | {
      type: 'subagent_snapshot';
      subagent: unknown;
      input_allowed: boolean;
    }
  | {
      type: 'subagent_event';
      sequence: number;
      parent_session_id: string;
      agent_id: string;
      event: unknown;
    }
  | {
      type: 'subagent_resync_required';
      skipped: number;
      subagent: unknown;
      input_allowed: boolean;
    }
  | {
      type: 'command_accepted';
      request_id: string;
      command: string;
      run_id?: string;
      queue_position?: number;
    }
  | {
      type: 'command_rejected';
      request_id: string;
      code: string;
      message: string;
    }
  | EventEnvelope
  | { type: 'resync_required'; skipped: number; session: SessionDto }
  | { type: 'pong'; request_id: string }
  | { type: 'fatal_error'; code: string; message: string };

/* -------------------------------------------------------------------------- */
/* Client commands                                                            */
/* -------------------------------------------------------------------------- */

export interface SkillInvocation {
  name: string;
  arguments?: string | null;
}

export type ClientCommand =
  | {
      type: 'prompt';
      request_id: string;
      content: Content;
      skill?: SkillInvocation;
    }
  | { type: 'stop'; request_id: string; run_id: string }
  | { type: 'compact'; request_id: string; instructions?: string | null }
  | { type: 'set_model'; request_id: string; model: string }
  | {
      type: 'set_reasoning_effort';
      request_id: string;
      effort: ReasoningEffort | null;
    }
  | { type: 'set_mode'; request_id: string; mode: AgentMode }
  | {
      type: 'answer_askuser';
      request_id: string;
      ask_id: string;
      answers: AskUserAnswer[];
    }
  | {
      type: 'decide_plan_approval';
      request_id: string;
      approval_id: string;
      decision: PlanApprovalDecision;
    }
  | { type: 'ping'; request_id: string };

/* -------------------------------------------------------------------------- */
/* HTTP response shapes                                                       */
/* -------------------------------------------------------------------------- */

export type ProviderKind = 'openai_chat' | 'openai_responses' | 'anthropic';

export interface PublicProviderConfig {
  profile_id: string;
  provider: ProviderKind;
  api_key_configured: boolean;
  base_url: string;
  model: string;
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

/** Body for `PUT /v1/providers/{profile_id}`. */
export interface PutProviderRequest {
  provider: ProviderKind;
  api_key: string;
  base_url: string;
  model: string;
  max_output_tokens?: number | null;
  max_context_tokens: number;
  temperature?: number | null;
  reasoning_effort?: ReasoningEffort | null;
  max_retries?: number;
  request_timeout_secs?: number;
  stream_idle_timeout_secs?: number;
}

export interface SessionSummary {
  session_id: string;
  profile_id: string;
  status: SessionStatus;
  active_run_id: string | null;
  queued_runs: number;
  mode: AgentMode | null;
  config: SessionConfig;
  message_count: number | null;
  subagents: SubagentSummary[];
}

export interface SessionsResponse {
  sessions: SessionSummary[];
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

export interface AuthTokenResponse {
  token: string;
  token_type: string;
  protocol: string;
  expires_in_secs: number;
}
