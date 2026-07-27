import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/platform/secure_storage.dart';
import 'package:phi_client/state/app_state.dart';
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

class _CountingSessionsTransport implements DaemonTransport {
  int listRequests = 0;

  @override
  String get displayName => 'test daemon';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    if (method != 'GET' || path != '/v1/sessions') {
      throw UnsupportedError('unexpected request: $method $path');
    }
    listRequests++;
    return const DaemonHttpResponse(
      200,
      '{"sessions":[{"session_id":"fixture-session",'
      '"title":"Fixture session","config":{}}],"workspaces":[]}',
      {},
    );
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

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({});
    debugSecureStorageOverride = _InMemorySecureStorage();
  });

  tearDown(() {
    debugSecureStorageOverride = null;
  });

  testWidgets('home loads the session list once without periodic polling', (
    tester,
  ) async {
    final settings = await AppSettings.load();
    await settings.addMachine(
      baseUrl: 'http://192.0.2.10:8787',
      authKey: 'fixture-key',
    );
    final transport = _CountingSessionsTransport();
    final app = AppState(settings, transportOverride: transport);

    await tester.pumpWidget(PhiApp(appState: app));
    await tester.pump();

    expect(transport.listRequests, 1);
    expect(find.text('Fixture session'), findsOneWidget);

    await tester.pump(const Duration(minutes: 1));
    expect(transport.listRequests, 1);

    await tester.pumpWidget(const SizedBox.shrink());
    app.dispose();
  });
}
