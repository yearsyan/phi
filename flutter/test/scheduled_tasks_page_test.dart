import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/scheduled_tasks_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Serves a fixed scheduled-task list and records mutations; everything else
/// throws (tests must not touch the network).
class _ScheduledTasksTransport implements DaemonTransport {
  _ScheduledTasksTransport(this.tasksJson);

  final List<Map<String, Object?>> tasksJson;

  String? putPath;
  Map<String, Object?>? putBody;

  @override
  String get displayName => 'fake';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    if (method == 'GET' && path == '/v1/scheduled-tasks') {
      return DaemonHttpResponse(
        200,
        jsonEncode({'tasks': tasksJson}),
        const {},
      );
    }
    if (method == 'GET' && path == '/v1/output-channels') {
      return DaemonHttpResponse(
        200,
        jsonEncode({'output_channels': const []}),
        const {},
      );
    }
    if (method == 'PUT' && path == '/v1/scheduled-tasks/task-1') {
      putPath = path;
      putBody = (body as Map).cast<String, Object?>();
      final updated = <String, Object?>{
        ...tasksJson.single,
        ...putBody!,
        'task_id': 'task-1',
        'revision': (putBody!['expected_revision'] as int? ?? 0) + 1,
      };
      return DaemonHttpResponse(200, jsonEncode(updated), const {});
    }
    throw UnsupportedError('unexpected request: $method $path');
  }

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async => throw UnsupportedError('sockets are not used by this test');

  @override
  void dispose() {}
}

Map<String, Object?> _dailyTaskJson() => {
  'task_id': 'task-1',
  'name': 'Morning review',
  'prompt': 'Summarize overnight failures',
  'workspace': '/workspace/phi',
  'profile_id': 'default',
  'agent_profile_id': 'default',
  'capability_mode': null,
  'output_channel_id': null,
  'schedule': {
    'type': 'daily',
    'time': '09:30',
    'weekdays': ['monday', 'tuesday', 'wednesday', 'thursday', 'friday'],
    'timezone': 'Asia/Shanghai',
  },
  'enabled': true,
  'created_at': '2026-07-20T00:00:00Z',
  'updated_at': '2026-07-20T00:00:00Z',
  'next_run_at': '2026-07-28T01:30:00Z',
  'last_run': null,
  'skipped_runs': 0,
  'revision': 7,
};

Future<AppState> _pumpPage(
  WidgetTester tester,
  _ScheduledTasksTransport transport,
) async {
  SharedPreferences.setMockInitialValues({});
  final settings = await AppSettings.load();
  final app = AppState(settings, transportOverride: transport);
  addTearDown(app.dispose);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh'), Locale('en')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: const ScheduledTasksPage(),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return app;
}

void main() {
  testWidgets('long daily schedule description does not overflow a phone '
      'screen', (tester) async {
    tester.view.physicalSize = const Size(360, 740);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    await _pumpPage(tester, _ScheduledTasksTransport([_dailyTaskJson()]));

    expect(find.text('Morning review'), findsOneWidget);
    expect(find.textContaining('09:30'), findsWidgets);
    // Any RenderFlex overflow would surface as a reported exception.
    expect(tester.takeException(), isNull);
  });

  testWidgets('task popup menu edits a task through the revision-aware PUT', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(360, 740);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);

    final transport = _ScheduledTasksTransport([_dailyTaskJson()]);
    await _pumpPage(tester, transport);

    await tester.tap(find.byType(PopupMenuButton<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑任务'));
    await tester.pumpAndSettle();

    expect(find.byType(AlertDialog), findsOneWidget);

    await tester.enterText(find.byType(TextField).first, 'Renamed review');
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();

    expect(transport.putPath, '/v1/scheduled-tasks/task-1');
    expect(transport.putBody, {
      'name': 'Renamed review',
      'prompt': 'Summarize overnight failures',
      'workspace': '/workspace/phi',
      'profile_id': 'default',
      'agent_profile_id': 'default',
      'capability_mode': null,
      'output_channel_id': null,
      'schedule': {
        'type': 'daily',
        'time': '09:30',
        'weekdays': ['monday', 'tuesday', 'wednesday', 'thursday', 'friday'],
        'timezone': 'Asia/Shanghai',
      },
      'expected_revision': 7,
    });
    expect(tester.takeException(), isNull);
  });
}
