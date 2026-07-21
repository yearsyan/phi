import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:phi_client/ui/widgets/workspace_picker.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Serves a fixed provider list for `GET /v1/providers`; everything else
/// throws (tests must not touch the network).
class _ProvidersTransport implements DaemonTransport {
  _ProvidersTransport(this.providersJson);

  final List<Map<String, Object?>> providersJson;

  @override
  String get displayName => 'fake';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    if (method == 'GET' && path == '/v1/providers') {
      return DaemonHttpResponse(
        200,
        jsonEncode({'providers': providersJson}),
        const {},
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
  testWidgets(
    'new session dialog falls back to a real profile when the daemon has '
    'no "default" profile',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final settings = await AppSettings.load();
      final app = AppState(
        settings,
        transportOverride: _ProvidersTransport([
          {'profile_id': 'work', 'provider': 'openai_chat', 'model': 'm-1'},
        ]),
      );
      addTearDown(app.dispose);

      NewSessionConfig? result;
      await tester.pumpWidget(
        AppScope(
          state: app,
          child: MaterialApp(
            locale: const Locale('zh'),
            supportedLocales: const [Locale('zh')],
            localizationsDelegates: GlobalMaterialLocalizations.delegates,
            theme: AppTheme.light(),
            home: Scaffold(
              body: Builder(
                builder: (context) => FilledButton(
                  onPressed: () async {
                    result = await showNewSessionDialog(context, app);
                  },
                  child: const Text('open'),
                ),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('open'));
      await tester.pump();
      // Let the providers future resolve and the fallback apply.
      await tester.pumpAndSettle();

      expect(find.text('work (m-1)'), findsOneWidget);

      await tester.tap(find.text('开始'));
      await tester.pumpAndSettle();

      // Must submit the daemon's real profile, not the stale 'default'.
      expect(result, isNotNull);
      expect(result!.profileId, 'work');
    },
  );
}
