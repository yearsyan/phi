import 'dart:convert';

import '../../core/models/wire.dart';
import '../../core/transport/daemon_transport.dart';

/// Typed convenience wrapper over the daemon's REST API.
///
/// All calls go through the injected [DaemonTransport], so this client works
/// unchanged over direct HTTP today and tunneled transports in the future.
class DaemonClient {
  DaemonClient(this.transport);

  final DaemonTransport transport;

  Future<Json> _requestJson(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    final response = await transport.request(
      method,
      path,
      query: query,
      body: body,
    );
    if (!response.isSuccess) {
      throw DaemonError.fromBody(response.statusCode, response.body);
    }
    if (response.body.isEmpty) return <String, dynamic>{};
    final decoded = jsonDecode(response.body);
    if (decoded is Map<String, dynamic>) return decoded;
    throw DaemonError('invalid_response', 'expected JSON object from $path');
  }

  /* --------------------------------- auth -------------------------------- */

  /// Mint a single-use, 60s WebSocket auth token.
  Future<String> mintSocketToken() async {
    final json = await _requestJson('POST', '/v1/auth/token');
    return json['token'] as String? ?? '';
  }

  /* ------------------------------- sessions ------------------------------ */

  Future<
    ({List<SessionSummary> sessions, List<WorkspaceSessionGroup> workspaces})
  >
  listSessions() async {
    final json = await _requestJson('GET', '/v1/sessions');
    final sessions = (json['sessions'] as List? ?? const [])
        .whereType<Map<String, dynamic>>()
        .map(SessionSummary.fromJson)
        .toList();
    final workspaces = (json['workspaces'] as List? ?? const [])
        .whereType<Map<String, dynamic>>()
        .map(WorkspaceSessionGroup.fromJson)
        .toList();
    return (sessions: sessions, workspaces: workspaces);
  }

  Future<SessionSummary> getSession(String sessionId) async {
    final json = await _requestJson('GET', '/v1/sessions/$sessionId');
    return SessionSummary.fromJson(json);
  }

  Future<SessionSummary> setPinned(String sessionId, bool pinned) async {
    final json = await _requestJson(
      'PATCH',
      '/v1/sessions/$sessionId',
      body: {'pinned': pinned},
    );
    return SessionSummary.fromJson(json);
  }

  Future<void> deleteSession(String sessionId) async {
    final response = await transport.request(
      'DELETE',
      '/v1/sessions/$sessionId',
    );
    if (!response.isSuccess && response.statusCode != 404) {
      throw DaemonError.fromBody(response.statusCode, response.body);
    }
  }

  Future<SessionSummary> forkSession(
    String sessionId,
    int messageIndex, {
    String position = 'after',
  }) async {
    final json = await _requestJson(
      'POST',
      '/v1/sessions/$sessionId/fork',
      body: {'message_index': messageIndex, 'position': position},
    );
    return SessionSummary.fromJson(json);
  }

  /* ------------------------------- providers ----------------------------- */

  Future<List<PublicProviderConfig>> listProviders() async {
    final json = await _requestJson('GET', '/v1/providers');
    return (json['providers'] as List? ?? const [])
        .whereType<Map<String, dynamic>>()
        .map(PublicProviderConfig.fromJson)
        .toList();
  }

  /* ------------------------------- workspace ----------------------------- */

  Future<WorkspaceBrowseResponse> browseWorkspace([String? path]) async {
    final json = await _requestJson(
      'GET',
      '/v1/workspaces/browse',
      query: path != null ? {'path': path} : null,
    );
    return WorkspaceBrowseResponse.fromJson(json);
  }

  /* ---------------------------- scheduled tasks -------------------------- */

  Future<List<ScheduledTask>> listScheduledTasks() async {
    final json = await _requestJson('GET', '/v1/scheduled-tasks');
    return (json['tasks'] as List? ?? const [])
        .whereType<Map<String, dynamic>>()
        .map(ScheduledTask.fromJson)
        .toList();
  }

  Future<ScheduledTask> createScheduledTask({
    required String name,
    required String prompt,
    String? workspace,
    String? profileId,
    String? agentProfileId,
    String? capabilityMode,
    required ScheduledTaskSchedule schedule,
  }) async {
    final json = await _requestJson(
      'POST',
      '/v1/scheduled-tasks',
      body: {
        'name': name,
        'prompt': prompt,
        'workspace': ?workspace,
        'profile_id': ?profileId,
        'agent_profile_id': ?agentProfileId,
        'capability_mode': ?capabilityMode,
        'schedule': schedule.toJson(),
      },
    );
    return ScheduledTask.fromJson(json);
  }

  Future<ScheduledTask> setScheduledTaskEnabled(
    String taskId,
    bool enabled,
    int expectedRevision,
  ) async {
    final json = await _requestJson(
      'PATCH',
      '/v1/scheduled-tasks/$taskId',
      body: {'enabled': enabled, 'expected_revision': expectedRevision},
    );
    return ScheduledTask.fromJson(json);
  }

  Future<void> deleteScheduledTask(String taskId) async {
    final response = await transport.request(
      'DELETE',
      '/v1/scheduled-tasks/$taskId',
    );
    if (!response.isSuccess) {
      throw DaemonError.fromBody(response.statusCode, response.body);
    }
  }

  Future<void> runScheduledTaskNow(String taskId) async {
    final response = await transport.request(
      'POST',
      '/v1/scheduled-tasks/$taskId/run',
    );
    if (!response.isSuccess) {
      throw DaemonError.fromBody(response.statusCode, response.body);
    }
  }
}
