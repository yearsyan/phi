import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:shared_preferences/shared_preferences.dart';

class _NoopTransport implements DaemonTransport {
  @override
  String get displayName => 'test';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    if (method == 'GET' && path == '/v1/sessions') {
      return const DaemonHttpResponse(
        200,
        '{"sessions":[],"workspaces":[]}',
        {},
      );
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

void main() {
  testWidgets('PhiApp applies appearance changes immediately', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final app = AppState(settings, transportOverride: _NoopTransport());
    try {
      await tester.pumpWidget(PhiApp(appState: app));
      await tester.pump();
      expect(
        tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
        ThemeMode.system,
      );

      await settings.setAppTheme('light');
      await tester.pump();
      expect(
        tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
        ThemeMode.light,
      );

      await settings.setAppTheme('dark');
      await tester.pump();
      expect(
        tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
        ThemeMode.dark,
      );
    } finally {
      await tester.pumpWidget(const SizedBox.shrink());
      app.dispose();
    }
  });
}
