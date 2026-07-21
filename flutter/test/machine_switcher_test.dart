import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/sessions_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<AppState> _pumpSessions(WidgetTester tester) async {
  SharedPreferences.setMockInitialValues({});
  final settings = await AppSettings.load();
  await settings.addMachine(
    name: 'Alpha',
    baseUrl: 'http://192.0.2.10:8787',
    authKey: 'fixture-key-alpha',
  );
  await settings.addMachine(
    name: 'Beta',
    baseUrl: 'http://192.0.2.20:8787',
    authKey: 'fixture-key-beta',
  );
  final app = AppState(settings);
  addTearDown(app.dispose);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh')],
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
  testWidgets('app bar shows the active machine name', (tester) async {
    await _pumpSessions(tester);

    expect(find.text('Alpha'), findsOneWidget);
    expect(find.text('Beta'), findsNothing);
  });

  testWidgets('switching machines updates settings and rebuilds the client', (
    tester,
  ) async {
    final app = await _pumpSessions(tester);
    final clientBefore = app.client;

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();

    // Sheet lists both machines plus the manage entry.
    expect(find.text('Beta'), findsOneWidget);
    expect(find.text('管理机器'), findsOneWidget);

    await tester.tap(find.text('Beta'));
    await tester.pumpAndSettle();

    expect(app.settings.activeMachine?.name, 'Beta');
    expect(app.client, isNot(same(clientBefore)));
    expect(find.text('Beta'), findsOneWidget);
  });

  testWidgets('manage entry opens the machines page', (tester) async {
    await _pumpSessions(tester);

    await tester.tap(find.text('Alpha'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('管理机器'));
    await tester.pumpAndSettle();

    expect(find.text('机器'), findsWidgets);
    expect(find.text('Beta'), findsOneWidget);
  });
}
