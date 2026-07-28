import 'package:flutter/foundation.dart';

import '../core/models/wire.dart';
import 'daemon_client.dart';

/// Holds the daemon-wide session list (flat + workspace-grouped) and scheduled
/// tasks. Refreshes are operation-driven; this store never polls.
class SessionsStore extends ChangeNotifier {
  SessionsStore(this._client);

  DaemonClient _client;

  List<SessionSummary> sessions = [];
  List<WorkspaceSessionGroup> workspaces = [];
  List<ScheduledTask> scheduledTasks = [];
  bool loading = false;
  Object? error;
  DateTime? lastLoadedAt;

  bool _disposed = false;
  bool _active = false;
  int _clientGeneration = 0;

  /// Performs the initial sessions request and enables automatic refreshes
  /// when the active daemon changes. Calling this more than once is a no-op.
  Future<void> activate() async {
    if (_disposed || _active) return;
    _active = true;
    await refresh();
  }

  /// Rebinds the store to a different daemon. If the sessions UI has already
  /// called [activate], a machine switch immediately loads the replacement
  /// daemon exactly once.
  ///
  /// The generation check in [refresh] prevents an in-flight response from
  /// the previous machine from overwriting the replacement machine's list.
  Future<void> replaceClient(DaemonClient client) async {
    if (_disposed) return;
    _client = client;
    _clientGeneration++;
    sessions = [];
    workspaces = [];
    scheduledTasks = [];
    loading = false;
    error = null;
    lastLoadedAt = null;
    notifyListeners();

    if (_active) await refresh();
  }

  Future<void> refresh({bool silent = false}) async {
    if (_disposed) return;
    final generation = _clientGeneration;
    final client = _client;
    if (!silent) {
      loading = true;
      notifyListeners();
    }
    try {
      final result = await client.listSessions();
      if (_disposed || generation != _clientGeneration) return;
      sessions = result.sessions;
      workspaces = result.workspaces;
      error = null;
      lastLoadedAt = DateTime.now();
    } catch (e) {
      if (_disposed || generation != _clientGeneration) return;
      error = e;
    } finally {
      if (!_disposed && generation == _clientGeneration) {
        loading = false;
        notifyListeners();
      }
    }
  }

  Future<void> refreshScheduledTasks() async {
    if (_disposed) return;
    try {
      scheduledTasks = await _client.listScheduledTasks();
      notifyListeners();
    } catch (_) {
      // Surface lazily on the tasks page instead.
    }
  }

  Future<SessionSummary> setPinned(String sessionId, bool pinned) async {
    final updated = await _client.setPinned(sessionId, pinned);
    _replaceSummary(updated);
    return updated;
  }

  Future<void> delete(String sessionId) async {
    await _client.deleteSession(sessionId);
    sessions.removeWhere((s) => s.sessionId == sessionId);
    for (final group in workspaces) {
      group.sessions.removeWhere((s) => s.sessionId == sessionId);
    }
    workspaces.removeWhere((g) => g.sessions.isEmpty);
    notifyListeners();
  }

  /// Projects the active run observed on an attached session socket into the
  /// global list, so the generation indicator changes without HTTP polling.
  void updateActiveRun(String sessionId, String? activeRunId) {
    var changed = false;
    for (var index = 0; index < sessions.length; index++) {
      final session = sessions[index];
      if (session.sessionId == sessionId &&
          session.activeRunId != activeRunId) {
        sessions[index] = _withActiveRunId(session, activeRunId);
        changed = true;
      }
    }
    for (final group in workspaces) {
      for (var index = 0; index < group.sessions.length; index++) {
        final session = group.sessions[index];
        if (session.sessionId == sessionId &&
            session.activeRunId != activeRunId) {
          group.sessions[index] = _withActiveRunId(session, activeRunId);
          changed = true;
        }
      }
    }
    if (changed) notifyListeners();
  }

  void _replaceSummary(SessionSummary updated) {
    final index = sessions.indexWhere((s) => s.sessionId == updated.sessionId);
    if (index >= 0) sessions[index] = updated;
    for (final group in workspaces) {
      final gi = group.sessions.indexWhere(
        (s) => s.sessionId == updated.sessionId,
      );
      if (gi >= 0) group.sessions[gi] = updated;
    }
    // Re-sort pinned-first within groups.
    sessions.sort(_compare);
    for (final group in workspaces) {
      group.sessions.sort(_compare);
    }
    notifyListeners();
  }

  static int _compare(SessionSummary a, SessionSummary b) {
    if (a.pinned != b.pinned) return a.pinned ? -1 : 1;
    return 0; // server already orders newest-first
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

SessionSummary _withActiveRunId(SessionSummary session, String? activeRunId) =>
    SessionSummary(
      sessionId: session.sessionId,
      title: session.title,
      pinned: session.pinned,
      profileId: session.profileId,
      workspace: session.workspace,
      status: session.status,
      activeRunId: activeRunId,
      queuedRuns: session.queuedRuns,
      capabilityMode: session.capabilityMode,
      config: session.config,
      messageCount: session.messageCount,
      subagents: session.subagents,
    );
