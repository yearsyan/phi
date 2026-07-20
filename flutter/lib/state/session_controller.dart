import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:uuid/uuid.dart';

import '../core/models/wire.dart';
import '../core/transport/daemon_transport.dart';
import 'daemon_client.dart';

/// Connection lifecycle of a session socket.
enum SessionConnectionPhase {
  idle,
  connecting,
  preparing,
  reconnecting,
  ready,
  error,
}

/// What this controller is bound to.
sealed class SessionTarget {
  const SessionTarget();
}

class NewSessionTarget extends SessionTarget {
  const NewSessionTarget({
    this.profileId = 'default',
    this.agentProfileId,
    this.capabilityMode,
    this.workspace,
  });

  final String profileId;
  final String? agentProfileId;
  final String? capabilityMode;
  final String? workspace;
}

class AttachSessionTarget extends SessionTarget {
  const AttachSessionTarget(this.sessionId);

  final String sessionId;
}

/* ------------------------------------------------------------------------- */
/* Activity steps (run "work detail")                                        */
/* ------------------------------------------------------------------------- */

sealed class ActivityStep {
  const ActivityStep();
}

class ToolStep extends ActivityStep {
  ToolStep({required this.call});

  ToolCall call;
  bool done = false;
  bool isError = false;
  String? content;
  final List<String> progress = [];
}

class NoticeStep extends ActivityStep {
  const NoticeStep(this.level, this.message);

  final String level; // info | warn | error
  final String message;
}

class SubagentStep extends ActivityStep {
  const SubagentStep(this.agentId, this.message, {this.detail});

  final String agentId;
  final String message;
  final String? detail;
}

class RetryStep extends ActivityStep {
  const RetryStep(this.retryNumber, this.maxRetries, this.reason);

  final int retryNumber;
  final int maxRetries;
  final String reason;
}

class TurnActivity {
  TurnActivity(this.turn);

  final int turn;
  final List<ActivityStep> steps = [];
  bool finished = false;

  int get toolCount => steps.whereType<ToolStep>().length;
}

class RunActivity {
  RunActivity({required this.runId, required this.historyStart});

  final String runId;
  final int historyStart;
  String status = 'running'; // queued | running | completed | stopped | failed
  String? errorMessage;
  final List<TurnActivity> turns = [];
}

class CompactionMarker {
  CompactionMarker({
    required this.key,
    required this.phase,
    required this.historyIndex,
    this.afterMessageCount,
    this.message,
  });

  final String key;
  String phase; // started | completed | failed
  final int historyIndex;
  int? afterMessageCount;
  String? message;
}

class PendingPrompt {
  PendingPrompt({
    required this.requestId,
    required this.content,
    this.matchAnyEcho = false,
  });

  final String requestId;
  final Content content;
  final bool matchAnyEcho;
  String status = 'sending'; // sending | accepted | queued
  int? queuePosition;
}

bool _sameContent(Content? left, Content? right) {
  if (left == null || right == null) return left == null && right == null;
  return jsonEncode(left.toJson()) == jsonEncode(right.toJson());
}

/* ------------------------------------------------------------------------- */
/* Controller                                                                */
/* ------------------------------------------------------------------------- */

/// Owns one durable app-to-session relationship: a WebSocket connection plus
/// the reduced session state projected from daemon frames.
///
/// A prepared (`/v1/ws/new`) session keeps its original socket after
/// `session_created`; if that socket later drops, retries attach to the
/// activated session id rather than creating a second session.
class SessionController extends ChangeNotifier {
  SessionController({
    required this.client,
    required this.target,
    this.onSessionListMayChange,
  });

  final DaemonClient client;
  final SessionTarget target;

  /// Called when the global session list should be refreshed (session created,
  /// title changed).
  final VoidCallback? onSessionListMayChange;

  static const _connectTimeout = Duration(seconds: 15);
  static const _reconnectDelaysMs = [800, 1600, 3200, 5000];
  static const _uuid = Uuid();

  /* ------------------------------ view state ----------------------------- */

  String? sessionId;
  String? title;
  String? workspace;
  String? profileId;
  bool ready = false;
  String status = SessionStatus.awaitingFirstPrompt;
  String capabilityMode = CapabilityMode.fullAccess;
  SessionConfig? config;
  List<SkillSummary> skills = [];
  Usage? usage;
  ContextUsage? contextUsage;
  String? activeRunId;
  int queuedRuns = 0;
  AssistantDraft? draft;
  List<AskUserRequest> pendingAsks = [];
  List<SubagentSummary> subagents = [];
  List<PublicMessage> history = [];
  List<CompactionMarker> compactions = [];
  RunActivity? activeRun;
  final List<PendingPrompt> pendingPrompts = [];
  ({String code, String message})? fatalError;
  final List<String> notices = [];
  int lastSequence = 0;
  bool resyncNeeded = false;

  SessionConnectionPhase phase = SessionConnectionPhase.idle;
  String? connectionError;

  /* --------------------------- connection state -------------------------- */

  DaemonSocket? _socket;
  StreamSubscription<String>? _subscription;
  String? _promotedSessionId;
  String? _preparedPromptRequestId;
  int _reconnectCount = 0;
  Timer? _reconnectTimer;
  Timer? _deadline;
  bool _disposed = false;
  int _connectionGeneration = 0;
  bool _started = false;

  bool get canSend =>
      phase == SessionConnectionPhase.ready && fatalError == null;

  bool get isBusy =>
      activeRunId != null ||
      status == SessionStatus.running ||
      status == SessionStatus.compacting;

  void start() {
    if (_started) return;
    _started = true;
    _openConnection();
  }

  @override
  void dispose() {
    _disposed = true;
    _connectionGeneration++;
    _deadline?.cancel();
    _reconnectTimer?.cancel();
    _closeSocket();
    super.dispose();
  }

  void _closeSocket() {
    final socket = _socket;
    _socket = null;
    _subscription?.cancel();
    _subscription = null;
    if (socket != null) {
      unawaited(socket.close());
    }
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  /* --------------------------- connection loop --------------------------- */

  Future<void> _openConnection() async {
    if (_disposed) return;
    final generation = ++_connectionGeneration;
    _deadline?.cancel();

    phase = _reconnectCount > 0
        ? SessionConnectionPhase.reconnecting
        : SessionConnectionPhase.connecting;
    connectionError = null;
    if (_reconnectCount == 0) {
      _resetState();
    }
    _notify();

    // Connect deadline.
    _deadline = Timer(_connectTimeout, () {
      if (_disposed || generation != _connectionGeneration) return;
      _scheduleReconnect(
        'Session connection timed out after ${_connectTimeout.inSeconds} seconds.',
        generation,
      );
    });

    try {
      final token = await client.mintSocketToken();
      if (_disposed || generation != _connectionGeneration) return;
      final protocols = ['phi.v1', 'phi.auth.$token'];

      final promoted = _promotedSessionId;
      final socket = promoted != null
          ? await client.transport.connect(
              '/v1/ws/attach/$promoted',
              protocols: protocols,
              timeout: _connectTimeout,
            )
          : switch (target) {
              NewSessionTarget(
                profileId: final profileId,
                agentProfileId: final agentProfileId,
                capabilityMode: final cap,
                workspace: final ws,
              ) =>
                await client.transport.connect(
                  '/v1/ws/new',
                  query: {
                    if (profileId != 'default') 'profile_id': profileId,
                    'agent_profile_id': ?agentProfileId,
                    'capability_mode': ?cap,
                    'workspace': ?ws,
                  },
                  protocols: protocols,
                  timeout: _connectTimeout,
                ),
              AttachSessionTarget(sessionId: final id) =>
                await client.transport.connect(
                  '/v1/ws/attach/$id',
                  protocols: protocols,
                  timeout: _connectTimeout,
                ),
            };

      if (_disposed || generation != _connectionGeneration) {
        await socket.close();
        return;
      }
      _socket = socket;
      if (phase == SessionConnectionPhase.connecting ||
          phase == SessionConnectionPhase.reconnecting) {
        phase = SessionConnectionPhase.preparing;
        _notify();
      }
      _subscription = socket.messages.listen(
        (frame) => _onFrame(frame, generation),
        onError: (Object error) {
          if (_disposed || generation != _connectionGeneration) return;
          _scheduleReconnect('Connection error: $error', generation);
        },
        onDone: () {
          if (_disposed || generation != _connectionGeneration) return;
          _scheduleReconnect('Session connection closed.', generation);
        },
      );
    } catch (error) {
      if (_disposed || generation != _connectionGeneration) return;
      _deadline?.cancel();
      if (_reconnectCount > 0) {
        _scheduleReconnect('$error', generation);
        return;
      }
      phase = SessionConnectionPhase.error;
      connectionError = '$error';
      _notify();
    }
  }

  void _scheduleReconnect(String message, int generation) {
    if (_disposed || generation != _connectionGeneration) return;
    if (_reconnectTimer != null) return; // already scheduled
    _deadline?.cancel();
    _closeSocket();
    _markDisconnected();

    // A first prompt may already be running on a /new socket that died before
    // session_created; never auto-resend it.
    if (target is NewSessionTarget &&
        _promotedSessionId == null &&
        _preparedPromptRequestId != null) {
      phase = SessionConnectionPhase.error;
      connectionError =
          '$message The first prompt may already be running; the client will not resend it automatically.';
      _notify();
      return;
    }

    if (_reconnectCount >= _reconnectDelaysMs.length) {
      phase = SessionConnectionPhase.error;
      connectionError = message;
      _notify();
      return;
    }

    final delay = _reconnectDelaysMs[_reconnectCount];
    _reconnectCount++;
    phase = SessionConnectionPhase.reconnecting;
    connectionError = message;
    _notify();
    _reconnectTimer = Timer(Duration(milliseconds: delay), () {
      _reconnectTimer = null;
      _openConnection();
    });
  }

  /// Manual retry from the UI.
  void retry() {
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _reconnectCount = 0;
    _openConnection();
  }

  /* ------------------------------ frame intake --------------------------- */

  void _onFrame(String raw, int generation) {
    if (_disposed || generation != _connectionGeneration) return;
    final frame = ServerFrame.parse(raw);

    if (frame.type == 'event') {
      final envelope = frame.envelope;
      if (envelope == null) return;
      if (envelope.sequence <= lastSequence) return;
      if (envelope.sequence != lastSequence + 1 || resyncNeeded) {
        _scheduleReconnect(
          'Session events became out of sync at sequence ${envelope.sequence}.',
          generation,
        );
        return;
      }
      lastSequence = envelope.sequence;
    } else if (frame.type == 'snapshot' || frame.type == 'resync_required') {
      final session = frame.session;
      if (session != null) lastSequence = session.lastSequence;
    }

    switch (frame.type) {
      case 'building':
        phase = SessionConnectionPhase.preparing;
        _notify();
        return;
      case 'session_created':
        _promotedSessionId = frame.sessionId;
        sessionId = frame.sessionId;
        _preparedPromptRequestId = null;
        onSessionListMayChange?.call();
        _notify();
        return;
      case 'ready':
      case 'snapshot':
      case 'resync_required':
        _deadline?.cancel();
        _reconnectCount = 0;
        phase = SessionConnectionPhase.ready;
        connectionError = null;
        break;
      case 'fatal_error':
        _deadline?.cancel();
        // Fatal errors are terminal for this connection.
        _connectionGeneration++;
        _closeSocket();
        _applyFatal(
          frame.code ?? 'fatal_error',
          frame.message ?? 'unknown error',
        );
        phase = SessionConnectionPhase.error;
        connectionError = frame.message;
        _notify();
        return;
      case 'command_rejected':
        if (frame.requestId == _preparedPromptRequestId) {
          _preparedPromptRequestId = null;
        }
        break;
    }

    _reduce(frame);
  }

  /* -------------------------------- reducer ------------------------------ */

  void _resetState() {
    sessionId = target is AttachSessionTarget
        ? (target as AttachSessionTarget).sessionId
        : null;
    title = null;
    workspace = target is NewSessionTarget
        ? (target as NewSessionTarget).workspace
        : null;
    profileId = target is NewSessionTarget
        ? (target as NewSessionTarget).profileId
        : null;
    ready = false;
    status = SessionStatus.awaitingFirstPrompt;
    capabilityMode = CapabilityMode.fullAccess;
    config = null;
    skills = [];
    usage = null;
    contextUsage = null;
    activeRunId = null;
    queuedRuns = 0;
    draft = null;
    pendingAsks = [];
    subagents = [];
    history = [];
    compactions = [];
    activeRun = null;
    pendingPrompts.clear();
    fatalError = null;
    notices.clear();
    lastSequence = 0;
    resyncNeeded = false;
  }

  void _reduce(ServerFrame frame) {
    switch (frame.type) {
      case 'ready':
        config = frame.readyConfig;
        capabilityMode = frame.readyCapabilityMode ?? capabilityMode;
        workspace = frame.readyWorkspace ?? workspace;
        skills = frame.readySkills;
        ready = true;
        break;
      case 'snapshot':
      case 'resync_required':
        final session = frame.session;
        if (session != null) _applySnapshot(session);
        break;
      case 'command_accepted':
        for (final prompt in pendingPrompts) {
          if (prompt.requestId == frame.requestId) {
            prompt.status = frame.queuePosition != null ? 'queued' : 'accepted';
            prompt.queuePosition = frame.queuePosition;
          }
        }
        break;
      case 'command_rejected':
        pendingPrompts.removeWhere((p) => p.requestId == frame.requestId);
        _pushNotice('${frame.code}: ${frame.message}');
        break;
      case 'event':
        final envelope = frame.envelope;
        if (envelope != null) _applyEvent(envelope.event, envelope.runId);
        break;
    }
    _notify();
  }

  void _applySnapshot(SessionDto session) {
    sessionId = session.sessionId;
    title = session.title;
    workspace = session.workspace;
    profileId = session.profileId;
    ready = true;
    status = session.status;
    capabilityMode = session.capabilityMode;
    config = session.config;
    skills = session.skills.isNotEmpty ? session.skills : skills;
    usage = session.usage;
    contextUsage = session.usage.context;
    activeRunId = session.activeRunId;
    queuedRuns = session.queuedRuns;
    draft = session.draft;
    pendingAsks = session.pendingAsks;
    subagents = session.subagents;
    history = List.of(session.history);
    compactions = [
      for (final (index, c) in session.contextCompactions.indexed)
        CompactionMarker(
          key: 'compaction-snapshot-${session.lastSequence}-$index',
          phase: c.phase,
          historyIndex: c.historyIndex.clamp(0, session.history.length).toInt(),
          afterMessageCount: c.afterMessageCount,
          message: c.message,
        ),
    ];
    activeRun = session.activeRunId == null
        ? null
        : RunActivity(
            runId: session.activeRunId!,
            historyStart: session.history.length,
          );
    pendingPrompts.clear();
    fatalError = null;
    resyncNeeded = false;
  }

  void _applyFatal(String code, String message) {
    ready = false;
    status = SessionStatus.offline;
    pendingPrompts.clear();
    fatalError = (code: code, message: message);
  }

  void _markDisconnected() {
    ready = false;
    pendingPrompts.clear();
    _notify();
  }

  void _pushNotice(String message) {
    notices.add(message);
  }

  void clearNotice(int index) {
    if (index >= 0 && index < notices.length) {
      notices.removeAt(index);
      _notify();
    }
  }

  /* ----------------------------- event reducer --------------------------- */

  void _applyEvent(WireEvent event, String? runId) {
    switch (event.type) {
      case 'state_changed':
        status = event.status ?? status;
      case 'session_initialized':
        break;
      case 'title_changed':
        title = event.title;
        onSessionListMayChange?.call();
      case 'run_queued':
        queuedRuns += 1;
      case 'run_started':
        _startRun(event.runId ?? '');
      case 'run_completed':
        _finalizeRun(event.runId ?? '', 'completed');
      case 'run_stopped':
        _finalizeRun(event.runId ?? '', 'stopped');
      case 'run_failed':
        _finalizeRun(event.runId ?? '', 'failed', event.message);
      case 'config_changed':
        config = event.config ?? config;
      case 'capability_mode_changed':
        capabilityMode = event.capabilityMode ?? capabilityMode;
      case 'askuser_requested':
        final request = event.askRequest;
        if (request != null) pendingAsks.add(request);
      case 'askuser_answered':
      case 'askuser_cancelled':
        pendingAsks.removeWhere((ask) => ask.askId == event.askId);
      case 'operation_failed':
        _pushNotice('${event.json['operation']}: ${event.message}');
      case 'actor_crashed':
        fatalError = (code: 'actor_crashed', message: event.message ?? '');
        status = SessionStatus.idle;
      case 'subagents_resynced':
        subagents = (event.json['subagents'] as List? ?? const [])
            .whereType<Map<String, dynamic>>()
            .map(SubagentSummary.fromJson)
            .toList();
      case 'agent_start':
      case 'agent_end':
      case 'agent_stopped':
        break;
      case 'message_start':
        _handleRoleMessage(event.messageDto);
      case 'message_update':
        final delta = event.delta;
        if (delta != null) {
          draft = _applyDelta(draft ?? const AssistantDraft(), delta);
        }
      case 'message_end':
        // User/tool messages are projected at message_start / turn_end; the
        // assistant message is committed atomically at turn_end.
        break;
      case 'message_aborted':
        draft = null;
      case 'turn_start':
        _ensureTurn(runId, event.turn ?? 1);
      case 'turn_end':
        _handleTurnEnd(event, runId);
      case 'tool_execution_start':
        final call = event.toolCall;
        if (call != null) {
          _recordToolStep(runId, call, (step) {
            step.done = false;
            step.content = null;
            step.isError = false;
          });
          final current = draft ?? const AssistantDraft();
          draft = current.copyWith(
            forkMessageIndex: () =>
                forkMessageIndex(history.length, compactions),
          );
        }
      case 'tool_execution_progress':
        final call = event.toolCall;
        final message = event.toolProgressMessage;
        if (call != null && message != null) {
          _recordToolStep(runId, call, (step) => step.progress.add(message));
        }
      case 'tool_execution_end':
        final call = event.toolCall;
        if (call != null) {
          _recordToolStep(runId, call, (step) {
            step.done = true;
            step.content = event.toolContent;
            step.isError = event.toolIsError;
          });
        }
      case 'subagent_spawned':
        _pushStep(
          runId,
          SubagentStep(
            event.agentId ?? '',
            'spawned subagent: ${event.json['description']}',
          ),
        );
      case 'subagent_state_changed':
        _pushStep(
          runId,
          SubagentStep(
            event.agentId ?? '',
            'subagent ${event.agentId} → ${event.json['state']}',
          ),
        );
      case 'subagent_notification':
        final notification = event.json['notification'];
        if (notification is Map<String, dynamic>) {
          _pushStep(
            runId,
            SubagentStep(
              event.agentId ?? '',
              notification['message'] as String? ?? '',
              detail: '${notification['kind']} (${notification['source']})',
            ),
          );
        }
      case 'subagent_run_finished':
        _pushStep(
          runId,
          SubagentStep(
            event.agentId ?? '',
            'subagent run ${event.json['run_id']} finished',
          ),
        );
      case 'subagent_closed':
        _pushStep(
          runId,
          SubagentStep(
            event.agentId ?? '',
            'subagent closed: ${event.json['reason']}',
          ),
        );
      case 'subagent_message_queued':
      case 'subagent_agent_event':
      case 'subagent_output_validated':
      case 'subagent_resource_finalized':
      case 'subagent_resource_finalization_failed':
        break; // too noisy to surface individually
      case 'provider_retry':
        _pushStep(
          runId,
          RetryStep(
            event.json['retry_number'] as int? ?? 0,
            event.json['max_retries'] as int? ?? 0,
            _retryReasonText(event.json['reason']),
          ),
        );
      case 'context_compaction_started':
        compactions.add(
          CompactionMarker(
            key: 'compaction-${lastSequence + 1}',
            phase: 'started',
            historyIndex: history.length,
          ),
        );
      case 'context_compaction_completed':
        _finishCompaction(
          'completed',
          event.json['after_message_count'] as int?,
        );
        draft = null;
        final completionUsage = event.usageDto;
        if (usage != null && completionUsage != null) {
          usage = Usage(
            cumulative: _addUsage(usage!.cumulative, completionUsage),
          );
        }
        contextUsage = null;
      case 'context_compaction_failed':
        _finishCompaction('failed', null, event.message);
      case 'usage_update':
        final eventUsage = event.usageDto;
        if (eventUsage != null) {
          usage = Usage(
            last: eventUsage,
            context: event.contextUsage,
            cumulative: _addUsage(
              usage?.cumulative ?? const TokenUsage(),
              eventUsage,
            ),
          );
          contextUsage = event.contextUsage;
        }
      case 'error':
        _pushNotice(event.message ?? 'unknown error');
    }
  }

  void _startRun(String runId) {
    activeRunId = runId;
    status = SessionStatus.running;
    queuedRuns = queuedRuns > 0 ? queuedRuns - 1 : 0;
    draft = null;
    activeRun = RunActivity(runId: runId, historyStart: history.length);
  }

  void _finalizeRun(String runId, String runStatus, [String? message]) {
    final run = activeRun;
    if (run == null || run.runId != runId || activeRunId != runId) {
      queuedRuns = queuedRuns > 0 ? queuedRuns - 1 : 0;
      return;
    }
    status = SessionStatus.idle;
    activeRunId = null;
    draft = null;
    run.status = runStatus;
    run.errorMessage = message;
    for (final turn in run.turns) {
      turn.finished = true;
    }
    onSessionListMayChange?.call();
  }

  void _ensureTurn(String? runId, int turnNumber) {
    final run = activeRun;
    if (run == null) return;
    if (runId != null && run.runId != runId) return;
    if (run.turns.any((t) => t.turn == turnNumber)) return;
    run.turns.add(TurnActivity(turnNumber));
  }

  TurnActivity? _currentTurn(String? runId) {
    final run = activeRun;
    if (run == null) return null;
    if (runId != null && run.runId != runId) return null;
    final unfinished = run.turns.where((t) => !t.finished);
    if (unfinished.isNotEmpty) return unfinished.first;
    if (run.turns.isNotEmpty) return run.turns.last;
    final created = TurnActivity(1);
    run.turns.add(created);
    return created;
  }

  void _recordToolStep(
    String? runId,
    ToolCall call,
    void Function(ToolStep step) mutate,
  ) {
    final turn = _currentTurn(runId);
    if (turn == null) return;
    ToolStep? step;
    for (final s in turn.steps.whereType<ToolStep>()) {
      if (s.call.id == call.id && s.call.name == call.name) {
        step = s;
        break;
      }
    }
    if (step == null) {
      step = ToolStep(call: call);
      turn.steps.add(step);
    } else {
      step.call = call; // refresh streamed args
    }
    mutate(step);
  }

  void _pushStep(String? runId, ActivityStep step) {
    final turn = _currentTurn(runId);
    turn?.steps.add(step);
  }

  void _finishCompaction(
    String phase,
    int? afterMessageCount, [
    String? message,
  ]) {
    CompactionMarker? started;
    for (final marker in compactions.reversed) {
      if (marker.phase == 'started') {
        started = marker;
        break;
      }
    }
    if (started == null) {
      compactions.add(
        CompactionMarker(
          key: 'compaction-${lastSequence + 1}',
          phase: phase,
          historyIndex: history.length,
          afterMessageCount: afterMessageCount,
          message: message,
        ),
      );
    } else {
      started.phase = phase;
      started.afterMessageCount = afterMessageCount;
      started.message = message;
    }
  }

  void _handleRoleMessage(PublicMessage? message) {
    if (message == null) return;
    if (message.role == 'user') {
      if (!message.isPublic) {
        // Internal payloads are redacted; keep a placeholder so later fork
        // indexes stay aligned with the daemon's transcript.
        history.add(message);
        return;
      }
      final exactIndex = pendingPrompts.indexWhere(
        (p) => _sameContent(p.content, message.content),
      );
      final pendingIndex = exactIndex >= 0
          ? exactIndex
          : pendingPrompts.indexWhere((p) => p.matchAnyEcho);
      final isDuplicate =
          pendingIndex < 0 &&
          history.isNotEmpty &&
          history.last.role == 'user' &&
          _sameContent(history.last.content, message.content);
      if (!isDuplicate) {
        history.add(message);
      }
      if (pendingIndex >= 0) {
        pendingPrompts.removeAt(pendingIndex);
      }
      return;
    }
    if (message.role == 'assistant') {
      draft = const AssistantDraft();
    }
  }

  void _handleTurnEnd(WireEvent event, String? runId) {
    final turnNumber = event.turn ?? 1;
    _ensureTurn(runId, turnNumber);
    final turn = _currentTurn(runId);
    turn?.finished = true;
    final message = event.messageDto;
    if (message != null && message.role == 'assistant') {
      history.add(message);
    }
    history.addAll(event.toolResults);
    draft = null;
  }

  static AssistantDraft _applyDelta(
    AssistantDraft draft,
    AssistantDelta delta,
  ) {
    switch (delta.kind) {
      case 'reasoning':
        return draft.copyWith(reasoning: draft.reasoning + (delta.delta ?? ''));
      case 'text':
        return draft.copyWith(text: draft.text + (delta.delta ?? ''));
      case 'tool_call':
        final toolCalls = List<ToolCallDraft>.of(draft.toolCalls);
        final index = toolCalls.indexWhere((t) => t.index == delta.index);
        if (index >= 0) {
          final existing = toolCalls[index];
          toolCalls[index] = existing.copyWith(
            id: delta.id != null ? () => delta.id : null,
            name: delta.name != null ? () => delta.name : null,
            arguments: existing.arguments + (delta.argumentsDelta ?? ''),
          );
        } else {
          toolCalls.add(
            ToolCallDraft(
              index: delta.index ?? 0,
              id: delta.id,
              name: delta.name,
              arguments: delta.argumentsDelta ?? '',
            ),
          );
          toolCalls.sort((a, b) => a.index.compareTo(b.index));
        }
        return draft.copyWith(toolCalls: toolCalls);
      default:
        return draft;
    }
  }

  static String _retryReasonText(Object? reason) {
    if (reason is! Map<String, dynamic>) return '';
    return switch (reason['type']) {
      'request_timeout' => 'request timeout (${reason['timeout_ms']} ms)',
      'transport' => 'transport error: ${reason['message']}',
      'http_status' => 'HTTP ${reason['status']}',
      _ => '',
    };
  }

  static TokenUsage _addUsage(TokenUsage left, TokenUsage right) => TokenUsage(
    inputTokens: left.inputTokens + right.inputTokens,
    outputTokens: left.outputTokens + right.outputTokens,
    totalTokens: left.totalTokens + right.totalTokens,
    cachedInputTokens: left.cachedInputTokens + right.cachedInputTokens,
  );

  /* ------------------------------- commands ------------------------------ */

  bool _send(ClientCommand command) {
    final socket = _socket;
    if (socket == null || phase == SessionConnectionPhase.error) {
      _pushNotice('The session connection is not open.');
      _notify();
      return false;
    }
    try {
      socket.send(command.encode());
      return true;
    } catch (error) {
      _pushNotice('Failed to send: $error');
      _notify();
      return false;
    }
  }

  static String _requestId(String prefix) =>
      '$prefix-${_uuid.v4().substring(0, 8)}';

  bool sendPrompt(String text, {String? skillName, String? skillArguments}) =>
      sendPromptContent(
        Content.text(text.trim()),
        skillName: skillName,
        skillArguments: skillArguments,
      );

  bool sendPromptContent(
    Content content, {
    String? skillName,
    String? skillArguments,
  }) {
    final trimmed = content.plainText.trim();
    final skill = skillName?.trim() ?? '';
    final hasAttachment =
        content.isParts && content.parts.any((part) => part.type != 'text');
    if (trimmed.isEmpty && !hasAttachment && skill.isEmpty) return false;
    final promptContent = content.isParts ? content : Content.text(trimmed);
    final requestId = _requestId('prompt');
    final sent = _send(
      ClientCommand.prompt(
        requestId,
        promptContent,
        skillName: skill.isEmpty ? null : skill,
        skillArguments: skillArguments?.trim().isEmpty ?? true
            ? null
            : skillArguments!.trim(),
      ),
    );
    if (sent) {
      if (target is NewSessionTarget && _promotedSessionId == null) {
        _preparedPromptRequestId = requestId;
      }
      pendingPrompts.add(
        PendingPrompt(
          requestId: requestId,
          content: skill.isEmpty
              ? promptContent
              : Content.text('/$skill${trimmed.isEmpty ? '' : ' $trimmed'}'),
          matchAnyEcho: skill.isNotEmpty,
        ),
      );
      _notify();
    }
    return sent;
  }

  void stop() {
    final runId = activeRunId;
    if (runId == null) return;
    _send(ClientCommand.stop(_requestId('stop'), runId));
  }

  bool answerAsk(String askId, List<Json> answers) =>
      _send(ClientCommand.answerAskUser(_requestId('answer'), askId, answers));

  void setModel(String model) {
    final value = model.trim();
    if (value.isEmpty) return;
    _send(ClientCommand.setModel(_requestId('model'), value));
  }

  void setReasoningEffort(String? effort) {
    _send(ClientCommand.setReasoningEffort(_requestId('reasoning'), effort));
  }

  void setCapabilityMode(String mode) {
    _send(ClientCommand.setCapabilityMode(_requestId('capability'), mode));
  }

  bool compact({String? instructions}) {
    final trimmed = instructions?.trim();
    return _send(
      ClientCommand.compact(
        _requestId('compact'),
        instructions: trimmed == null || trimmed.isEmpty ? null : trimmed,
      ),
    );
  }
}

/// Maps a retained display-history position into the active transcript,
/// accounting for completed compactions.
int? forkMessageIndex(int historyIndex, List<CompactionMarker> compactions) {
  CompactionMarker? latest;
  for (final marker in compactions) {
    if (marker.phase == 'completed') latest = marker;
  }
  if (latest == null) return historyIndex;
  if (historyIndex < latest.historyIndex || latest.afterMessageCount == null) {
    return null;
  }
  return latest.afterMessageCount! + (historyIndex - latest.historyIndex);
}
