import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/daemon_client.dart';

void main() {
  test(
    'scheduled-task replacement sends a complete revision-aware PUT',
    () async {
      final transport = _RecordingTransport();
      final client = DaemonClient(transport);
      final schedule = ScheduledTaskSchedule.interval(
        every: 30,
        unit: 'minutes',
      );

      final updated = await client.replaceScheduledTask(
        taskId: 'task-1',
        name: 'Edited task',
        prompt: 'Review failures',
        workspace: '/workspace/phi',
        profileId: 'default',
        agentProfileId: 'reviewer',
        capabilityMode: 'workspace_edit',
        outputChannelId: null,
        schedule: schedule,
        expectedRevision: 3,
      );

      expect(transport.method, 'PUT');
      expect(transport.path, '/v1/scheduled-tasks/task-1');
      expect(transport.body, {
        'name': 'Edited task',
        'prompt': 'Review failures',
        'workspace': '/workspace/phi',
        'profile_id': 'default',
        'agent_profile_id': 'reviewer',
        'capability_mode': 'workspace_edit',
        'output_channel_id': null,
        'schedule': {'type': 'interval', 'every': 30, 'unit': 'minutes'},
        'expected_revision': 3,
      });
      expect(updated.name, 'Edited task');
      expect(updated.revision, 4);
    },
  );
}

class _RecordingTransport implements DaemonTransport {
  String? method;
  String? path;
  Object? body;

  @override
  String get displayName => 'recording';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    this.method = method;
    this.path = path;
    this.body = body;
    return DaemonHttpResponse(
      200,
      jsonEncode({
        'task_id': 'task-1',
        'name': 'Edited task',
        'prompt': 'Review failures',
        'workspace': '/workspace/phi',
        'profile_id': 'default',
        'agent_profile_id': 'reviewer',
        'capability_mode': 'workspace_edit',
        'output_channel_id': null,
        'schedule': {'type': 'interval', 'every': 30, 'unit': 'minutes'},
        'enabled': true,
        'created_at': '2026-07-25T00:00:00Z',
        'updated_at': '2026-07-25T00:10:00Z',
        'next_run_at': '2026-07-25T00:40:00Z',
        'last_run': null,
        'skipped_runs': 0,
        'revision': 4,
      }),
      const {},
    );
  }

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async => throw UnsupportedError('not used');

  @override
  void dispose() {}
}
