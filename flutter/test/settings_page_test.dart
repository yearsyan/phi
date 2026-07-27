import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/app_licenses.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/settings_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<AppSettings> _pumpSettings(WidgetTester tester) async {
  SharedPreferences.setMockInitialValues({});
  final settings = await AppSettings.load();
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
        home: const SettingsPage(),
      ),
    ),
  );
  await tester.pump();
  return settings;
}

void main() {
  testWidgets('open-source licenses are reachable from settings', (
    tester,
  ) async {
    registerBundledAssetLicenses();
    await _pumpSettings(tester);

    final licenses = find.text('开源许可证');
    await tester.scrollUntilVisible(licenses, 200);
    await tester.tap(licenses);
    await tester.pumpAndSettle();

    expect(find.byType(LicensePage), findsOneWidget);
  });

  testWidgets('appearance can be switched to light mode', (tester) async {
    final settings = await _pumpSettings(tester);
    final appearanceField = find.byWidgetPredicate(
      (widget) =>
          widget is DropdownButtonFormField<String> &&
          widget.decoration.labelText == '外观',
    );

    expect(appearanceField, findsOneWidget);
    await tester.tap(appearanceField);
    await tester.pumpAndSettle();
    await tester.tap(find.text('浅色').last);
    await tester.pumpAndSettle();

    expect(settings.appTheme, 'light');
    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getString('ui.app_theme'), 'light');
  });
}
