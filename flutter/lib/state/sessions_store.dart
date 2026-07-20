import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/models/wire.dart';
import 'daemon_client.dart';

/// Holds the daemon-wide session list (flat + workspace-grouped) and scheduled
/// tasks, polling periodically and on demand.
class SessionsStore extends ChangeNotifier {
  SessionsStore(this._client);

  final DaemonClient _client;

  List<SessionSummary> sessions = [];
  List<WorkspaceSessionGroup> workspaces = [];
  List<ScheduledTask> scheduledTasks = [];
  bool loading = false;
  Object? error;
  DateTime? lastLoadedAt;

  Timer? _pollTimer;
  bool _disposed = false;

  /// Start polling (call when the sessions UI is visible).
  void startPolling({Duration interval = const Duration(seconds: 8)}) {
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(interval, (_) => refresh(silent: true));
    unawaited(refresh());
  }

  void stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  Future<void> refresh({bool silent = false}) async {
    if (_disposed) return;
    if (!silent) {
      loading = true;
      notifyListeners();
    }
    try {
      final result = await _client.listSessions();
      if (_disposed) return;
      sessions = result.sessions;
      workspaces = result.workspaces;
      error = null;
      lastLoadedAt = DateTime.now();
    } catch (e) {
      if (_disposed) return;
      error = e;
    } finally {
      if (!_disposed) {
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
    stopPolling();
    super.dispose();
  }
}
