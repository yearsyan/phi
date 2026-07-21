import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/ui/pages/machines_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

Future<AppState> _pumpApp(WidgetTester tester, {int machineCount = 0}) async {
  SharedPreferences.setMockInitialValues({});
  final settings = await AppSettings.load();
  for (var i = 0; i < machineCount; i++) {
    await settings.addMachine(
      name: 'Machine $i',
      baseUrl: 'http://192.0.2.1$i:8787',
      authKey: 'fixture-key-$i',
    );
  }
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
        home: const MachinesPage(),
      ),
    ),
  );
  await tester.pump();
  return app;
}

void main() {
  testWidgets('empty state offers to add a machine', (tester) async {
    await _pumpApp(tester);

    expect(find.text('尚未配置机器'), findsOneWidget);
    expect(find.text('添加机器'), findsOneWidget);
  });

  testWidgets('adding a machine through the editor lists and activates it', (
    tester,
  ) async {
    final app = await _pumpApp(tester);

    await tester.tap(find.text('添加机器'));
    await tester.pumpAndSettle();

    // Editor: name, URL, key fields in order.
    await tester.enterText(find.byType(TextField).at(0), 'Workstation');
    await tester.enterText(find.byType(TextField).at(1), '192.0.2.10:8787');
    await tester.enterText(find.byType(TextField).at(2), 'fixture-key-new');
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pumpAndSettle();

    expect(app.settings.machines, hasLength(1));
    expect(app.settings.activeMachine, isNotNull);
    expect(app.settings.activeMachine!.displayName, 'Workstation');
    expect(find.text('Workstation'), findsOneWidget);
    expect(find.text('当前使用'), findsOneWidget);
  });

  testWidgets('saving without URL/key shows a validation message', (
    tester,
  ) async {
    final app = await _pumpApp(tester);

    await tester.tap(find.text('添加机器'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, '保存'));
    await tester.pump();

    expect(find.text('地址和密钥必填。'), findsOneWidget);
    expect(app.settings.machines, isEmpty);
  });

  testWidgets('machines are listed with the active badge', (tester) async {
    await _pumpApp(tester, machineCount: 2);

    expect(find.text('Machine 0'), findsOneWidget);
    expect(find.text('Machine 1'), findsOneWidget);
    expect(find.text('当前使用'), findsOneWidget);
  });

  testWidgets('set active from the actions sheet switches the machine', (
    tester,
  ) async {
    final app = await _pumpApp(tester, machineCount: 2);
    final firstId = app.settings.machines.first.id;
    final secondId = app.settings.machines.last.id;
    expect(app.settings.activeMachine?.id, firstId);

    await tester.longPress(find.text('Machine 1'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('设为当前'));
    await tester.pumpAndSettle();

    expect(app.settings.activeMachine?.id, secondId);
  });

  testWidgets('deleting the active machine confirms and clears selection', (
    tester,
  ) async {
    final app = await _pumpApp(tester, machineCount: 1);

    await tester.longPress(find.text('Machine 0'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();

    expect(find.text('删除机器？'), findsOneWidget);
    expect(find.textContaining('删除后应用将回到未配置状态'), findsOneWidget);

    await tester.tap(find.widgetWithText(FilledButton, '删除'));
    await tester.pumpAndSettle();

    expect(app.settings.machines, isEmpty);
    expect(app.settings.activeMachine, isNull);
    expect(find.text('尚未配置机器'), findsOneWidget);
  });
}
