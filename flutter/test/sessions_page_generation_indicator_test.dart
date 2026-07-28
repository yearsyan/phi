import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/platform/secure_storage.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/sessions_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

class _InMemorySecureStorage implements SecureKeyValueStore {
  final Map<String, String> _values = {};

  @override
  Future<String?> read(String key) async => _values[key];

  @override
  Future<void> write(String key, String value) async {
    _values[key] = value;
  }

  @override
  Future<void> delete(String key) async {
    _values.remove(key);
  }
}

const _config = SessionConfig(model: 'test-model', revision: 1);

SessionSummary _session({
  required String id,
  required String title,
  String status = SessionStatus.idle,
  String? activeRunId,
}) => SessionSummary(
  sessionId: id,
  title: title,
  status: status,
  activeRunId: activeRunId,
  config: _config,
);

Future<AppState> _pumpSessionsPage(WidgetTester tester) async {
  SharedPreferences.setMockInitialValues({});
  final settings = await AppSettings.load();
  await settings.addMachine(
    name: 'Test daemon',
    baseUrl: 'http://192.0.2.10:8787',
    authKey: 'fixture-key',
  );
  final app = AppState(settings);
  addTearDown(app.dispose);

  final idle = _session(id: 'idle', title: 'Idle session');
  final offline = _session(
    id: 'offline',
    title: 'Offline session',
    status: SessionStatus.offline,
  );
  final generating = _session(
    id: 'generating',
    title: 'Generating session',
    status: SessionStatus.running,
    activeRunId: 'run-1',
  );
  app.sessionsStore.sessions = [idle, offline, generating];
  app.sessionsStore.workspaces = [
    WorkspaceSessionGroup(
      workspace: '/workspace/phi',
      sessions: [idle, offline, generating],
    ),
  ];

  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        locale: const Locale('en'),
        supportedLocales: const [Locale('en')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: SessionsPage(
          embedded: false,
          selectedSessionId: null,
          onOpenSession: (_) {},
          onNewSession: () {},
          onOpenTasks: () {},
          onOpenSettings: () {},
        ),
      ),
    ),
  );
  await tester.pump();
  return app;
}

void main() {
  setUp(() {
    debugSecureStorageOverride = _InMemorySecureStorage();
  });

  tearDown(() {
    debugSecureStorageOverride = null;
  });

  testWidgets('session list shows a dot only while a run is active', (
    tester,
  ) async {
    final app = await _pumpSessionsPage(tester);

    expect(find.byKey(const ValueKey('session-generating-idle')), findsNothing);
    expect(
      find.byKey(const ValueKey('session-generating-offline')),
      findsNothing,
    );
    expect(
      find.byKey(const ValueKey('session-generating-generating')),
      findsOneWidget,
    );

    final indicator = tester.widget<Container>(
      find.byKey(const ValueKey('session-generating-generating')),
    );
    expect(
      (indicator.decoration! as BoxDecoration).color,
      AppTheme.lightAccent,
    );

    app.sessionsStore.updateActiveRun('idle', 'run-2');
    app.sessionsStore.updateActiveRun('generating', null);
    await tester.pump();

    expect(
      find.byKey(const ValueKey('session-generating-idle')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('session-generating-generating')),
      findsNothing,
    );
  });
}
