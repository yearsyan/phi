import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/platform/qr_scan_support.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/sessions_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:phi_client/ui/widgets/machine_editor.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<AppState> _pumpApp(WidgetTester tester, Widget home) async {
  SharedPreferences.setMockInitialValues({});
  final app = AppState(await AppSettings.load());
  addTearDown(app.dispose);
  await tester.pumpWidget(
    MaterialApp(
      locale: const Locale('zh'),
      supportedLocales: const [Locale('zh')],
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      theme: AppTheme.light(),
      home: AppScope(state: app, child: home),
    ),
  );
  await tester.pump();
  return app;
}

void main() {
  // The scan entries are mobile-only; the test host (macOS/Linux) would hide
  // them, so force the support flag per test.
  tearDown(() => debugQrScanSupportedOverride = null);

  testWidgets(
    'machine editor shows the scan button when scanning is supported',
    (tester) async {
      debugQrScanSupportedOverride = true;
      await _pumpApp(tester, const MachineEditorPage());

      expect(find.byIcon(Icons.qr_code_scanner), findsOneWidget);
    },
  );

  testWidgets('machine editor buttons do not overflow on narrow screens', (
    tester,
  ) async {
    debugQrScanSupportedOverride = true;
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(320, 700);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    // A RenderFlex overflow would throw a FlutterError and fail this test.
    await _pumpApp(tester, const MachineEditorPage());

    expect(find.byIcon(Icons.qr_code_scanner), findsOneWidget);
  });

  testWidgets('machine editor hides the scan button when unsupported', (
    tester,
  ) async {
    debugQrScanSupportedOverride = false;
    await _pumpApp(tester, const MachineEditorPage());

    expect(find.byIcon(Icons.qr_code_scanner), findsNothing);
  });

  testWidgets('unconfigured sessions page offers scan to connect', (
    tester,
  ) async {
    debugQrScanSupportedOverride = true;
    await _pumpApp(
      tester,
      SessionsPage(
        embedded: false,
        selectedSessionId: null,
        onOpenSession: (_) {},
        onNewSession: () {},
        onOpenTasks: () {},
        onOpenSettings: () {},
      ),
    );

    expect(find.text('尚未配置 daemon'), findsOneWidget);
    expect(find.text('打开设置'), findsOneWidget);
    expect(find.text('扫码连接'), findsOneWidget);
  });

  testWidgets('unconfigured sessions page hides scan entry when unsupported', (
    tester,
  ) async {
    debugQrScanSupportedOverride = false;
    await _pumpApp(
      tester,
      SessionsPage(
        embedded: false,
        selectedSessionId: null,
        onOpenSession: (_) {},
        onNewSession: () {},
        onOpenTasks: () {},
        onOpenSettings: () {},
      ),
    );

    expect(find.text('尚未配置 daemon'), findsOneWidget);
    expect(find.text('扫码连接'), findsNothing);
  });
}
