/// Dart mirror of the phi-daemon wire protocol.
///
/// Derived from `crates/phi-daemon/src/api/dto.rs` and the web client's
/// `web/src/types/wire.ts`. Stable DTOs are modeled as full classes; the
/// large event union is modeled as [WireEvent] (type tag + raw JSON with
/// typed accessors) to stay robust against additive schema changes.
library;

import 'dart:convert';

/* ------------------------------------------------------------------------- */
/* Content / messages                                                        */
/* ------------------------------------------------------------------------- */

typedef Json = Map<String, dynamic>;

Json _asJson(Object? value) =>
    value is Map<String, dynamic> ? value : <String, dynamic>{};

List<Json> _asJsonList(Object? value) =>
    value is List ? value.map(_asJson).toList() : const [];

int _asInt(Object? value, [int fallback = 0]) =>
    value is int ? value : (value is num ? value.toInt() : fallback);

class Content {
  const Content.text(this.textValue) : isParts = false, parts = const [];
  const Content.parts(this.parts) : isParts = true, textValue = null;

  final bool isParts;
  final String? textValue;
  final List<ContentPart> parts;

  String get plainText => isParts
      ? parts.where((p) => p.type == 'text').map((p) => p.text ?? '').join('\n')
      : (textValue ?? '');

  static Content? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) return null;
    if (json['type'] == 'text') {
      return Content.text(json['value'] as String? ?? '');
    }
    if (json['type'] == 'parts') {
      return Content.parts(
        _asJsonList(json['value']).map(ContentPart.fromJson).toList(),
      );
    }
    return null;
  }

  Json toJson() => isParts
      ? {'type': 'parts', 'value': parts.map((p) => p.toJson()).toList()}
      : {'type': 'text', 'value': textValue ?? ''};
}

class ContentPart {
  const ContentPart({required this.type, this.text, this.raw});

  const ContentPart.text(String value)
    : type = 'text',
      text = value,
      raw = null;

  ContentPart.imageUrl(String url, {String? detail})
    : type = 'image_url',
      text = null,
      raw = {
        'type': 'image_url',
        'image_url': {'url': url, 'detail': ?detail},
      };

  final String type;
  final String? text;
  final Json? raw;

  String? get imageUrl {
    final image = raw?['image_url'];
    return image is Map ? image['url'] as String? : null;
  }

  static ContentPart fromJson(Json json) => ContentPart(
    type: json['type'] as String? ?? '',
    text: json['text'] as String?,
    raw: json,
  );

  Json toJson() => raw ?? {'type': type, if (text != null) 'text': text};
}

class ToolCall {
  const ToolCall({required this.id, required this.name, this.arguments});

  final String id;
  final String name;

  /// Arbitrary JSON arguments (decoded). May be null.
  final Object? arguments;

  static ToolCall fromJson(Json json) => ToolCall(
    id: json['id'] as String? ?? '',
    name: json['name'] as String? ?? '',
    arguments: json['arguments'],
  );

  String argumentsPretty() {
    final args = arguments;
    if (args == null) return '';
    try {
      return const JsonEncoder.withIndent('  ').convert(args);
    } catch (_) {
      return args.toString();
    }
  }

  /// Best-effort one-line summary of the call for list rows.
  String summary() {
    final args = arguments;
    if (args is Map) {
      for (final key in const [
        'path',
        'file_path',
        'command',
        'query',
        'pattern',
        'url',
        'description',
        'prompt',
      ]) {
        final v = args[key];
        if (v is String && v.isNotEmpty) {
          final oneLine = v.replaceAll('\n', ' ').trim();
          return oneLine.length > 120
              ? '${oneLine.substring(0, 120)}…'
              : oneLine;
        }
      }
    }
    final flat = argumentsPretty().replaceAll('\n', ' ');
    return flat.length > 120 ? '${flat.substring(0, 120)}…' : flat;
  }
}

class PublicMessage {
  const PublicMessage({
    required this.role,
    this.visibility = 'public',
    this.content,
    this.reasoning,
    this.toolCalls = const [],
    this.toolCallId,
    this.toolResultIsError = false,
    this.toolResultMetadata,
  });

  final String role; // system | user | assistant | tool
  final String visibility;
  final Content? content;
  final String? reasoning;
  final List<ToolCall> toolCalls;
  final String? toolCallId;
  final bool toolResultIsError;
  final Object? toolResultMetadata;

  bool get isPublic => visibility != 'internal';

  static PublicMessage fromJson(Json json) => PublicMessage(
    role: json['role'] as String? ?? 'assistant',
    visibility: json['visibility'] as String? ?? 'public',
    content: Content.fromJson(json['content']),
    reasoning: json['reasoning'] as String?,
    toolCalls: _asJsonList(json['tool_calls']).map(ToolCall.fromJson).toList(),
    toolCallId: json['tool_call_id'] as String?,
    toolResultIsError: json['tool_result_is_error'] as bool? ?? false,
    toolResultMetadata: json['tool_result_metadata'],
  );
}

/* ------------------------------------------------------------------------- */
/* Enums as string constants                                                 */
/* ------------------------------------------------------------------------- */

class SessionStatus {
  static const awaitingFirstPrompt = 'awaiting_first_prompt';
  static const idle = 'idle';
  static const compacting = 'compacting';
  static const running = 'running';
  static const stopping = 'stopping';
  static const closing = 'closing';
  static const closed = 'closed';
  static const offline = 'offline';

  static bool isBusy(String status) =>
      status == running || status == compacting || status == stopping;
}

class CapabilityMode {
  static const readOnly = 'read_only';
  static const workspaceEdit = 'workspace_edit';
  static const fullAccess = 'full_access';

  static const all = [readOnly, workspaceEdit, fullAccess];

  static String label(String mode) => switch (mode) {
    readOnly => 'Read only',
    workspaceEdit => 'Workspace edit',
    fullAccess => 'Full access',
    _ => mode,
  };
}

class ReasoningEffort {
  static const all = [
    'none',
    'minimal',
    'low',
    'medium',
    'high',
    'xhigh',
    'max',
  ];
}

/* ------------------------------------------------------------------------- */
/* Session DTOs                                                              */
/* ------------------------------------------------------------------------- */

class AgentProfileRef {
  const AgentProfileRef({required this.agentProfileId, required this.revision});

  final String agentProfileId;
  final int revision;

  static AgentProfileRef fromJson(Json json) => AgentProfileRef(
    agentProfileId: json['agent_profile_id'] as String? ?? 'default',
    revision: _asInt(json['revision']),
  );
}

class SessionConfig {
  const SessionConfig({
    required this.model,
    this.reasoningEffort,
    required this.revision,
  });

  final String model;
  final String? reasoningEffort;
  final int revision;

  static SessionConfig fromJson(Json json) => SessionConfig(
    model: json['model'] as String? ?? '',
    reasoningEffort: json['reasoning_effort'] as String?,
    revision: _asInt(json['revision']),
  );

  SessionConfig copyWith({
    String? model,
    String? Function()? reasoningEffort,
    int? revision,
  }) => SessionConfig(
    model: model ?? this.model,
    reasoningEffort: reasoningEffort != null
        ? reasoningEffort()
        : this.reasoningEffort,
    revision: revision ?? this.revision,
  );
}

class TokenUsage {
  const TokenUsage({
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.totalTokens = 0,
    this.cachedInputTokens = 0,
  });

  final int inputTokens;
  final int outputTokens;
  final int totalTokens;
  final int cachedInputTokens;

  static TokenUsage fromJson(Json json) => TokenUsage(
    inputTokens: _asInt(json['input_tokens']),
    outputTokens: _asInt(json['output_tokens']),
    totalTokens: _asInt(json['total_tokens']),
    cachedInputTokens: _asInt(json['cached_input_tokens']),
  );
}

class ContextUsage {
  const ContextUsage({
    required this.maxTokens,
    required this.usedTokens,
    required this.remainingTokens,
  });

  final int maxTokens;
  final int usedTokens;
  final int remainingTokens;

  double get fraction => maxTokens == 0 ? 0 : usedTokens / maxTokens;

  static ContextUsage fromJson(Json json) => ContextUsage(
    maxTokens: _asInt(json['max_tokens']),
    usedTokens: _asInt(json['used_tokens']),
    remainingTokens: _asInt(json['remaining_tokens']),
  );
}

class Usage {
  const Usage({this.last, this.context, this.cumulative = const TokenUsage()});

  final TokenUsage? last;
  final ContextUsage? context;
  final TokenUsage cumulative;

  static Usage fromJson(Json json) => Usage(
    last: json['last'] is Map
        ? TokenUsage.fromJson(_asJson(json['last']))
        : null,
    context: json['context'] is Map
        ? ContextUsage.fromJson(_asJson(json['context']))
        : null,
    cumulative: TokenUsage.fromJson(_asJson(json['cumulative'])),
  );
}

class ToolCallDraft {
  const ToolCallDraft({
    required this.index,
    this.id,
    this.name,
    this.arguments = '',
  });

  final int index;
  final String? id;
  final String? name;
  final String arguments;

  static ToolCallDraft fromJson(Json json) => ToolCallDraft(
    index: _asInt(json['index']),
    id: json['id'] as String?,
    name: json['name'] as String?,
    arguments: json['arguments'] as String? ?? '',
  );

  ToolCallDraft copyWith({
    String? Function()? id,
    String? Function()? name,
    String? arguments,
  }) => ToolCallDraft(
    index: index,
    id: id != null ? id() : this.id,
    name: name != null ? name() : this.name,
    arguments: arguments ?? this.arguments,
  );
}

class AssistantDraft {
  const AssistantDraft({
    this.reasoning = '',
    this.text = '',
    this.toolCalls = const [],
    this.forkMessageIndex,
  });

  final String reasoning;
  final String text;
  final List<ToolCallDraft> toolCalls;
  final int? forkMessageIndex;

  static AssistantDraft fromJson(Json json) => AssistantDraft(
    reasoning: json['reasoning'] as String? ?? '',
    text: json['text'] as String? ?? '',
    toolCalls: _asJsonList(
      json['tool_calls'],
    ).map(ToolCallDraft.fromJson).toList(),
    forkMessageIndex: json['fork_message_index'] is int
        ? json['fork_message_index'] as int
        : null,
  );

  AssistantDraft copyWith({
    String? reasoning,
    String? text,
    List<ToolCallDraft>? toolCalls,
    int? Function()? forkMessageIndex,
  }) => AssistantDraft(
    reasoning: reasoning ?? this.reasoning,
    text: text ?? this.text,
    toolCalls: toolCalls ?? this.toolCalls,
    forkMessageIndex: forkMessageIndex != null
        ? forkMessageIndex()
        : this.forkMessageIndex,
  );
}

class AskUserOption {
  const AskUserOption({required this.label, this.description, this.preview});

  final String label;
  final String? description;
  final String? preview;

  static AskUserOption fromJson(Json json) => AskUserOption(
    label: json['label'] as String? ?? '',
    description: json['description'] as String?,
    preview: json['preview'] as String?,
  );
}

class AskUserQuestion {
  const AskUserQuestion({
    required this.question,
    required this.header,
    this.options = const [],
    this.multiSelect = false,
  });

  final String question;
  final String header;
  final List<AskUserOption> options;
  final bool multiSelect;

  static AskUserQuestion fromJson(Json json) => AskUserQuestion(
    question: json['question'] as String? ?? '',
    header: json['header'] as String? ?? '',
    options: _asJsonList(json['options']).map(AskUserOption.fromJson).toList(),
    multiSelect: json['multiSelect'] as bool? ?? false,
  );
}

class AskUserRequest {
  const AskUserRequest({required this.askId, this.questions = const []});

  final String askId;
  final List<AskUserQuestion> questions;

  static AskUserRequest fromJson(Json json) => AskUserRequest(
    askId: json['ask_id'] as String? ?? '',
    questions: _asJsonList(
      json['questions'],
    ).map(AskUserQuestion.fromJson).toList(),
  );
}

class ToolPermissionRule {
  const ToolPermissionRule({required this.toolName, this.pattern});

  final String toolName;
  final String? pattern;

  static ToolPermissionRule fromJson(Json json) => ToolPermissionRule(
    toolName: json['tool_name'] as String? ?? '',
    pattern: json['pattern'] as String?,
  );

  Json toJson() => {
    'tool_name': toolName,
    if (pattern != null) 'pattern': pattern,
  };

  String get label => pattern == null ? toolName : '$toolName($pattern)';
}

class ToolPermissionPrompt {
  const ToolPermissionPrompt({
    required this.permissionId,
    required this.call,
    required this.effect,
    required this.capabilityMode,
    this.suggestions = const [],
  });

  final String permissionId;
  final ToolCall call;
  final String effect;
  final String capabilityMode;
  final List<ToolPermissionRule> suggestions;

  static ToolPermissionPrompt fromJson(Json json) => ToolPermissionPrompt(
    permissionId: json['permission_id'] as String? ?? '',
    call: ToolCall.fromJson(_asJson(json['call'])),
    effect: json['effect'] as String? ?? '',
    capabilityMode: json['capability_mode'] as String? ?? '',
    suggestions: _asJsonList(
      json['suggestions'],
    ).map(ToolPermissionRule.fromJson).toList(),
  );
}

class SubagentSummary {
  const SubagentSummary({
    required this.agentId,
    required this.description,
    required this.state,
    required this.lastSequence,
    required this.observerPath,
  });

  final String agentId;
  final String description;
  final String state;
  final int lastSequence;
  final String observerPath;

  static SubagentSummary fromJson(Json json) => SubagentSummary(
    agentId: json['agent_id'] as String? ?? '',
    description: json['description'] as String? ?? '',
    state: json['state'] as String? ?? '',
    lastSequence: _asInt(json['last_sequence']),
    observerPath: json['observer_path'] as String? ?? '',
  );
}

class ContextCompactionStatus {
  const ContextCompactionStatus({
    required this.phase,
    required this.historyIndex,
    this.afterMessageCount,
    this.message,
  });

  final String phase; // started | completed | failed
  final int historyIndex;
  final int? afterMessageCount;
  final String? message;

  static ContextCompactionStatus fromJson(Json json) => ContextCompactionStatus(
    phase: json['phase'] as String? ?? '',
    historyIndex: _asInt(json['history_index']),
    afterMessageCount: json['after_message_count'] is int
        ? json['after_message_count'] as int
        : null,
    message: json['message'] as String?,
  );
}

class SkillSummary {
  const SkillSummary({
    required this.name,
    this.displayName,
    this.description = '',
    this.argumentHint,
    this.modelInvocable = true,
    this.userInvocable = false,
  });

  final String name;
  final String? displayName;
  final String description;
  final String? argumentHint;
  final bool modelInvocable;
  final bool userInvocable;

  static SkillSummary fromJson(Json json) => SkillSummary(
    name: json['name'] as String? ?? '',
    displayName: json['display_name'] as String?,
    description: json['description'] as String? ?? '',
    argumentHint: json['argument_hint'] as String?,
    modelInvocable: json['model_invocable'] as bool? ?? true,
    userInvocable: json['user_invocable'] as bool? ?? false,
  );
}

class SessionDto {
  const SessionDto({
    required this.sessionId,
    this.title,
    this.profileId = 'default',
    this.agentProfile = const AgentProfileRef(
      agentProfileId: 'default',
      revision: 0,
    ),
    this.workspace,
    this.initialized = false,
    this.status = SessionStatus.offline,
    this.activeRunId,
    this.queuedRuns = 0,
    this.capabilityMode = CapabilityMode.workspaceEdit,
    required this.config,
    this.history = const [],
    this.contextCompactions = const [],
    this.draft,
    this.pendingAsks = const [],
    this.pendingToolPermissions = const [],
    this.skills = const [],
    this.subagents = const [],
    this.usage = const Usage(),
    this.lastSequence = 0,
  });

  final String sessionId;
  final String? title;
  final String profileId;
  final AgentProfileRef agentProfile;
  final String? workspace;
  final bool initialized;
  final String status;
  final String? activeRunId;
  final int queuedRuns;
  final String capabilityMode;
  final SessionConfig config;
  final List<PublicMessage> history;
  final List<ContextCompactionStatus> contextCompactions;
  final AssistantDraft? draft;
  final List<AskUserRequest> pendingAsks;
  final List<ToolPermissionPrompt> pendingToolPermissions;
  final List<SkillSummary> skills;
  final List<SubagentSummary> subagents;
  final Usage usage;
  final int lastSequence;

  static SessionDto fromJson(Json json) => SessionDto(
    sessionId: json['session_id'] as String? ?? '',
    title: json['title'] as String?,
    profileId: json['profile_id'] as String? ?? 'default',
    agentProfile: AgentProfileRef.fromJson(_asJson(json['agent_profile'])),
    workspace: json['workspace'] as String?,
    initialized: json['initialized'] as bool? ?? false,
    status: json['status'] as String? ?? SessionStatus.offline,
    activeRunId: json['active_run_id'] as String?,
    queuedRuns: _asInt(json['queued_runs']),
    capabilityMode:
        json['capability_mode'] as String? ?? CapabilityMode.workspaceEdit,
    config: SessionConfig.fromJson(_asJson(json['config'])),
    history: _asJsonList(json['history']).map(PublicMessage.fromJson).toList(),
    contextCompactions:
        (json['context_compactions'] is List
                ? _asJsonList(json['context_compactions'])
                : (json['context_compaction'] is Map
                      ? [_asJson(json['context_compaction'])]
                      : const <Json>[]))
            .map(ContextCompactionStatus.fromJson)
            .toList(),
    draft: json['draft'] is Map
        ? AssistantDraft.fromJson(_asJson(json['draft']))
        : null,
    pendingAsks: _asJsonList(
      json['pending_asks'],
    ).map(AskUserRequest.fromJson).toList(),
    pendingToolPermissions: _asJsonList(
      json['pending_tool_permissions'],
    ).map(ToolPermissionPrompt.fromJson).toList(),
    skills: _asJsonList(json['skills']).map(SkillSummary.fromJson).toList(),
    subagents: _asJsonList(
      json['subagents'],
    ).map(SubagentSummary.fromJson).toList(),
    usage: Usage.fromJson(_asJson(json['usage'])),
    lastSequence: _asInt(json['last_sequence']),
  );

  SessionDto copyWith({
    String? Function()? title,
    String? status,
    String? Function()? activeRunId,
    int? queuedRuns,
    String? capabilityMode,
    SessionConfig? config,
    List<PublicMessage>? history,
    List<ContextCompactionStatus>? contextCompactions,
    AssistantDraft? Function()? draft,
    List<AskUserRequest>? pendingAsks,
    List<ToolPermissionPrompt>? pendingToolPermissions,
    List<SkillSummary>? skills,
    List<SubagentSummary>? subagents,
    Usage? usage,
    int? lastSequence,
    bool? initialized,
  }) => SessionDto(
    sessionId: sessionId,
    title: title != null ? title() : this.title,
    profileId: profileId,
    agentProfile: agentProfile,
    workspace: workspace,
    initialized: initialized ?? this.initialized,
    status: status ?? this.status,
    activeRunId: activeRunId != null ? activeRunId() : this.activeRunId,
    queuedRuns: queuedRuns ?? this.queuedRuns,
    capabilityMode: capabilityMode ?? this.capabilityMode,
    config: config ?? this.config,
    history: history ?? this.history,
    contextCompactions: contextCompactions ?? this.contextCompactions,
    draft: draft != null ? draft() : this.draft,
    pendingAsks: pendingAsks ?? this.pendingAsks,
    pendingToolPermissions:
        pendingToolPermissions ?? this.pendingToolPermissions,
    skills: skills ?? this.skills,
    subagents: subagents ?? this.subagents,
    usage: usage ?? this.usage,
    lastSequence: lastSequence ?? this.lastSequence,
  );
}

class SessionSummary {
  const SessionSummary({
    required this.sessionId,
    this.title,
    this.pinned = false,
    this.profileId = 'default',
    this.workspace,
    this.status = SessionStatus.offline,
    this.activeRunId,
    this.queuedRuns = 0,
    this.capabilityMode,
    required this.config,
    this.messageCount,
    this.subagents = const [],
  });

  final String sessionId;
  final String? title;
  final bool pinned;
  final String profileId;
  final String? workspace;
  final String status;
  final String? activeRunId;
  final int queuedRuns;
  final String? capabilityMode;
  final SessionConfig config;
  final int? messageCount;
  final List<SubagentSummary> subagents;

  static SessionSummary fromJson(Json json) => SessionSummary(
    sessionId: json['session_id'] as String? ?? '',
    title: json['title'] as String?,
    pinned: json['pinned'] as bool? ?? false,
    profileId: json['profile_id'] as String? ?? 'default',
    workspace: json['workspace'] as String?,
    status: json['status'] as String? ?? SessionStatus.offline,
    activeRunId: json['active_run_id'] as String?,
    queuedRuns: _asInt(json['queued_runs']),
    capabilityMode: json['capability_mode'] as String?,
    config: SessionConfig.fromJson(_asJson(json['config'])),
    messageCount: json['message_count'] is int
        ? json['message_count'] as int
        : null,
    subagents: _asJsonList(
      json['subagents'],
    ).map(SubagentSummary.fromJson).toList(),
  );
}

class WorkspaceSessionGroup {
  const WorkspaceSessionGroup({this.workspace, this.sessions = const []});

  final String? workspace;
  final List<SessionSummary> sessions;

  static WorkspaceSessionGroup fromJson(Json json) => WorkspaceSessionGroup(
    workspace: json['workspace'] as String?,
    sessions: _asJsonList(
      json['sessions'],
    ).map(SessionSummary.fromJson).toList(),
  );
}

/* ------------------------------------------------------------------------- */
/* Scheduled tasks                                                           */
/* ------------------------------------------------------------------------- */

class ScheduledTaskSchedule {
  const ScheduledTaskSchedule.daily({
    required this.time,
    required this.weekdays,
    required this.timezone,
  }) : type = 'daily',
       every = null,
       unit = null;

  const ScheduledTaskSchedule.interval({
    required int this.every,
    required String this.unit,
  }) : type = 'interval',
       time = null,
       weekdays = const [],
       timezone = null;

  final String type; // daily | interval
  final String? time;
  final List<String> weekdays;
  final String? timezone;
  final int? every;
  final String? unit;

  static ScheduledTaskSchedule fromJson(Json json) {
    if (json['type'] == 'daily') {
      return ScheduledTaskSchedule.daily(
        time: json['time'] as String? ?? '09:00',
        weekdays: (json['weekdays'] as List? ?? const [])
            .map((e) => e.toString())
            .toList(),
        timezone: json['timezone'] as String? ?? 'UTC',
      );
    }
    return ScheduledTaskSchedule.interval(
      every: _asInt(json['every'], 1),
      unit: json['unit'] as String? ?? 'hours',
    );
  }

  Json toJson() => type == 'daily'
      ? {
          'type': 'daily',
          'time': time,
          'weekdays': weekdays,
          'timezone': timezone,
        }
      : {'type': 'interval', 'every': every, 'unit': unit};

  String describe() => type == 'daily'
      ? 'Daily $time (${weekdays.join(', ')}) [$timezone]'
      : 'Every $every $unit';
}

class ScheduledTaskRun {
  const ScheduledTaskRun({
    required this.scheduledFor,
    this.startedAt,
    this.finishedAt,
    required this.outcome,
    this.sessionId,
    this.error,
  });

  final String scheduledFor;
  final String? startedAt;
  final String? finishedAt;
  final String outcome; // running | succeeded | failed | stopped | interrupted
  final String? sessionId;
  final String? error;

  static ScheduledTaskRun fromJson(Json json) => ScheduledTaskRun(
    scheduledFor: json['scheduled_for'] as String? ?? '',
    startedAt: json['started_at'] as String?,
    finishedAt: json['finished_at'] as String?,
    outcome: json['outcome'] as String? ?? '',
    sessionId: json['session_id'] as String?,
    error: json['error'] as String?,
  );
}

class ScheduledTask {
  const ScheduledTask({
    required this.taskId,
    required this.name,
    required this.prompt,
    this.workspace,
    this.profileId,
    this.agentProfileId,
    this.capabilityMode,
    required this.schedule,
    this.enabled = true,
    this.createdAt,
    this.updatedAt,
    this.nextRunAt,
    this.lastRun,
    this.skippedRuns = 0,
    this.revision = 0,
  });

  final String taskId;
  final String name;
  final String prompt;
  final String? workspace;
  final String? profileId;
  final String? agentProfileId;
  final String? capabilityMode;
  final ScheduledTaskSchedule schedule;
  final bool enabled;
  final String? createdAt;
  final String? updatedAt;
  final String? nextRunAt;
  final ScheduledTaskRun? lastRun;
  final int skippedRuns;
  final int revision;

  static ScheduledTask fromJson(Json json) => ScheduledTask(
    taskId: json['task_id'] as String? ?? '',
    name: json['name'] as String? ?? '',
    prompt: json['prompt'] as String? ?? '',
    workspace: json['workspace'] as String?,
    profileId: json['profile_id'] as String?,
    agentProfileId: json['agent_profile_id'] as String?,
    capabilityMode: json['capability_mode'] as String?,
    schedule: ScheduledTaskSchedule.fromJson(_asJson(json['schedule'])),
    enabled: json['enabled'] as bool? ?? true,
    createdAt: json['created_at'] as String?,
    updatedAt: json['updated_at'] as String?,
    nextRunAt: json['next_run_at'] as String?,
    lastRun: json['last_run'] is Map
        ? ScheduledTaskRun.fromJson(_asJson(json['last_run']))
        : null,
    skippedRuns: _asInt(json['skipped_runs']),
    revision: _asInt(json['revision']),
  );
}

/* ------------------------------------------------------------------------- */
/* Providers                                                                 */
/* ------------------------------------------------------------------------- */

/// Public provider profile (never includes the API key).
class PublicProviderConfig {
  const PublicProviderConfig({
    required this.profileId,
    required this.provider,
    this.apiKeyConfigured = false,
    this.baseUrl = '',
    this.model = '',
    this.maxOutputTokens,
    this.maxContextTokens = 0,
    this.reasoningEffort,
    this.revision = 0,
  });

  final String profileId;
  final String provider; // openai_chat | openai_responses | anthropic
  final bool apiKeyConfigured;
  final String baseUrl;
  final String model;
  final int? maxOutputTokens;
  final int maxContextTokens;
  final String? reasoningEffort;
  final int revision;

  static PublicProviderConfig fromJson(Json json) => PublicProviderConfig(
    profileId: json['profile_id'] as String? ?? 'default',
    provider: json['provider'] as String? ?? '',
    apiKeyConfigured: json['api_key_configured'] as bool? ?? false,
    baseUrl: json['base_url'] as String? ?? '',
    model: json['model'] as String? ?? '',
    maxOutputTokens: json['max_output_tokens'] is int
        ? json['max_output_tokens'] as int
        : null,
    maxContextTokens: _asInt(json['max_context_tokens']),
    reasoningEffort: json['reasoning_effort'] as String?,
    revision: _asInt(json['revision']),
  );
}

/* ------------------------------------------------------------------------- */
/* Workspace browsing                                                        */
/* ------------------------------------------------------------------------- */

class WorkspaceDirectory {
  const WorkspaceDirectory({required this.name, required this.path});

  final String name;
  final String path;

  static WorkspaceDirectory fromJson(Json json) => WorkspaceDirectory(
    name: json['name'] as String? ?? '',
    path: json['path'] as String? ?? '',
  );
}

class WorkspaceBrowseResponse {
  const WorkspaceBrowseResponse({
    required this.path,
    this.parent,
    this.directories = const [],
    this.truncated = false,
  });

  final String path;
  final String? parent;
  final List<WorkspaceDirectory> directories;
  final bool truncated;

  static WorkspaceBrowseResponse fromJson(Json json) => WorkspaceBrowseResponse(
    path: json['path'] as String? ?? '/',
    parent: json['parent'] as String?,
    directories: _asJsonList(
      json['directories'],
    ).map(WorkspaceDirectory.fromJson).toList(),
    truncated: json['truncated'] as bool? ?? false,
  );
}

/* ------------------------------------------------------------------------- */
/* Events                                                                    */
/* ------------------------------------------------------------------------- */

/// A daemon runtime event (`EventDto`). The union is large and additive, so
/// it is modeled as a type tag over the raw JSON with typed accessors for
/// the fields the client consumes.
class WireEvent {
  const WireEvent(this.type, this.json);

  final String type;
  final Json json;

  static WireEvent fromJson(Json json) =>
      WireEvent(json['type'] as String? ?? 'unknown', json);

  String? get runId => json['run_id'] as String?;
  String? get status => json['status'] as String?;
  String? get title => json['title'] as String?;
  String? get message => json['message'] as String?;
  String? get askId => json['ask_id'] as String?;
  String? get permissionId => json['permission_id'] as String?;
  String? get capabilityMode => json['capability_mode'] as String?;
  String? get agentId => json['agent_id'] as String?;

  SessionConfig? get config => json['config'] is Map
      ? SessionConfig.fromJson(_asJson(json['config']))
      : null;

  PublicMessage? get messageDto => json['message'] is Map
      ? PublicMessage.fromJson(_asJson(json['message']))
      : null;

  List<PublicMessage> get toolResults =>
      _asJsonList(json['tool_results']).map(PublicMessage.fromJson).toList();

  AskUserRequest? get askRequest => json['request'] is Map
      ? AskUserRequest.fromJson(_asJson(json['request']))
      : null;

  ToolPermissionPrompt? get toolPermissionRequest => json['request'] is Map
      ? ToolPermissionPrompt.fromJson(_asJson(json['request']))
      : null;

  ToolCall? get toolCall =>
      json['call'] is Map ? ToolCall.fromJson(_asJson(json['call'])) : null;

  String? get toolContent => json['content'] as String?;
  bool get toolIsError => json['is_error'] as bool? ?? false;
  String? get toolProgressMessage => json['progress'] is Map
      ? _asJson(json['progress'])['message'] as String?
      : null;

  ContextUsage? get contextUsage => json['context_usage'] is Map
      ? ContextUsage.fromJson(_asJson(json['context_usage']))
      : null;

  TokenUsage? get usageDto =>
      json['usage'] is Map ? TokenUsage.fromJson(_asJson(json['usage'])) : null;

  /// Parse the `delta` payload of a `message_update` event.
  AssistantDelta? get delta {
    final d = json['delta'];
    if (d is! Map<String, dynamic>) return null;
    return AssistantDelta.fromJson(d);
  }

  SubagentSummary? get subagentSummary => null;

  int? get turn => json['turn'] is int ? json['turn'] as int : null;

  @override
  String toString() => 'WireEvent($type)';
}

class AssistantDelta {
  const AssistantDelta.reasoning(this.delta)
    : kind = 'reasoning',
      index = null,
      id = null,
      name = null,
      argumentsDelta = null;

  const AssistantDelta.text(this.delta)
    : kind = 'text',
      index = null,
      id = null,
      name = null,
      argumentsDelta = null;

  const AssistantDelta.toolCall({
    required int this.index,
    this.id,
    this.name,
    required String this.argumentsDelta,
  }) : kind = 'tool_call',
       delta = null;

  final String kind; // reasoning | text | tool_call
  final String? delta;
  final int? index;
  final String? id;
  final String? name;
  final String? argumentsDelta;

  static AssistantDelta fromJson(Json json) {
    switch (json['type']) {
      case 'reasoning':
        return AssistantDelta.reasoning(json['delta'] as String? ?? '');
      case 'text':
        return AssistantDelta.text(json['delta'] as String? ?? '');
      case 'tool_call':
        return AssistantDelta.toolCall(
          index: _asInt(json['index']),
          id: json['id'] as String?,
          name: json['name'] as String?,
          argumentsDelta: json['arguments_delta'] as String? ?? '',
        );
      default:
        return const AssistantDelta.text('');
    }
  }
}

/// An envelope carrying an ordered session event.
class EventEnvelope {
  const EventEnvelope({
    required this.sequence,
    required this.sessionId,
    this.runId,
    required this.event,
  });

  final int sequence;
  final String sessionId;
  final String? runId;
  final WireEvent event;
}

/* ------------------------------------------------------------------------- */
/* Server frames                                                             */
/* ------------------------------------------------------------------------- */

/// Parsed top-level WebSocket frame from the daemon (`ServerMessage`).
class ServerFrame {
  const ServerFrame._(this.type, this.json);

  final String type;
  final Json json;

  static ServerFrame parse(String raw) {
    final json = jsonDecode(raw);
    if (json is! Map<String, dynamic>) {
      return const ServerFrame._('unknown', {});
    }
    return ServerFrame._(json['type'] as String? ?? 'unknown', json);
  }

  // ready
  SessionConfig? get readyConfig => json['config'] is Map
      ? SessionConfig.fromJson(_asJson(json['config']))
      : null;
  String? get readyCapabilityMode => json['capability_mode'] as String?;
  String? get readyWorkspace => json['workspace'] as String?;
  List<SkillSummary> get readySkills =>
      _asJsonList(json['skills']).map(SkillSummary.fromJson).toList();

  // session_created
  String? get sessionId => json['session_id'] as String?;

  // snapshot / resync_required
  SessionDto? get session => json['session'] is Map
      ? SessionDto.fromJson(_asJson(json['session']))
      : null;
  int get skipped => _asInt(json['skipped']);

  // event
  EventEnvelope? get envelope {
    if (type != 'event') return null;
    return EventEnvelope(
      sequence: _asInt(json['sequence']),
      sessionId: json['session_id'] as String? ?? '',
      runId: json['run_id'] as String?,
      event: WireEvent.fromJson(_asJson(json['event'])),
    );
  }

  // command_accepted / command_rejected / pong
  String? get requestId => json['request_id'] as String?;
  String? get command => json['command'] as String?;
  String? get code => json['code'] as String?;
  String? get message => json['message'] as String?;
  String? get runId => json['run_id'] as String?;
  int? get queuePosition =>
      json['queue_position'] is int ? json['queue_position'] as int : null;
}

/* ------------------------------------------------------------------------- */
/* Client commands                                                           */
/* ------------------------------------------------------------------------- */

/// Outgoing client command (`ClientCommand`).
class ClientCommand {
  ClientCommand._(this.json);

  final Json json;

  String encode() => jsonEncode(json);

  factory ClientCommand.prompt(
    String requestId,
    Content content, {
    String? skillName,
    String? skillArguments,
  }) => ClientCommand._({
    'type': 'prompt',
    'request_id': requestId,
    'content': content.toJson(),
    if (skillName != null)
      'skill': {'name': skillName, 'arguments': ?skillArguments},
  });

  factory ClientCommand.stop(String requestId, String runId) => ClientCommand._(
    {'type': 'stop', 'request_id': requestId, 'run_id': runId},
  );

  factory ClientCommand.compact(String requestId, {String? instructions}) =>
      ClientCommand._({
        'type': 'compact',
        'request_id': requestId,
        'instructions': ?instructions,
      });

  factory ClientCommand.setModel(String requestId, String model) =>
      ClientCommand._({
        'type': 'set_model',
        'request_id': requestId,
        'model': model,
      });

  factory ClientCommand.setReasoningEffort(String requestId, String? effort) =>
      ClientCommand._({
        'type': 'set_reasoning_effort',
        'request_id': requestId,
        'effort': effort,
      });

  factory ClientCommand.setCapabilityMode(String requestId, String mode) =>
      ClientCommand._({
        'type': 'set_capability_mode',
        'request_id': requestId,
        'capability_mode': mode,
      });

  factory ClientCommand.answerAskUser(
    String requestId,
    String askId,
    List<Json> answers,
  ) => ClientCommand._({
    'type': 'answer_askuser',
    'request_id': requestId,
    'ask_id': askId,
    'answers': answers,
  });

  factory ClientCommand.decideToolPermission(
    String requestId,
    String permissionId,
    Json decision,
  ) => ClientCommand._({
    'type': 'decide_tool_permission',
    'request_id': requestId,
    'permission_id': permissionId,
    'decision': decision,
  });

  factory ClientCommand.ping(String requestId) =>
      ClientCommand._({'type': 'ping', 'request_id': requestId});
}

/// Daemon error response body: `{"code": "...", "message": "..."}`.
class DaemonError implements Exception {
  const DaemonError(this.code, this.message, [this.statusCode]);

  final String code;
  final String message;
  final int? statusCode;

  static DaemonError fromBody(int statusCode, String body) {
    try {
      final json = jsonDecode(body);
      if (json is Map<String, dynamic>) {
        return DaemonError(
          json['code'] as String? ?? 'error',
          json['message'] as String? ?? body,
          statusCode,
        );
      }
    } catch (_) {}
    return DaemonError(
      'http_$statusCode',
      body.isEmpty ? 'HTTP $statusCode' : body,
      statusCode,
    );
  }

  bool get isUnauthorized => code == 'unauthorized' || statusCode == 401;

  @override
  String toString() => 'DaemonError($code): $message';
}
