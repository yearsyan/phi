import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/ui/widgets/macos_window_frame.dart';

void main() {
  tearDown(() {
    debugMacosWindowTargetPlatformOverride = null;
  });

  testWidgets('reserves the native macOS title-bar control strip', (
    tester,
  ) async {
    debugMacosWindowTargetPlatformOverride = TargetPlatform.macOS;

    await tester.pumpWidget(
      const MediaQuery(
        data: MediaQueryData(),
        child: MacosWindowFrame(child: SizedBox(key: Key('content'))),
      ),
    );

    final context = tester.element(find.byKey(const Key('content')));
    expect(
      MediaQuery.paddingOf(context).top,
      MacosWindowFrame.titleBarSafeInset,
    );
    expect(
      MediaQuery.viewPaddingOf(context).top,
      MacosWindowFrame.titleBarSafeInset,
    );
  });

  testWidgets('preserves a larger system-provided top inset', (tester) async {
    debugMacosWindowTargetPlatformOverride = TargetPlatform.macOS;
    const existingInset = 42.0;

    await tester.pumpWidget(
      const MediaQuery(
        data: MediaQueryData(
          padding: EdgeInsets.only(top: existingInset),
          viewPadding: EdgeInsets.only(top: existingInset),
        ),
        child: MacosWindowFrame(child: SizedBox(key: Key('content'))),
      ),
    );

    final context = tester.element(find.byKey(const Key('content')));
    expect(MediaQuery.paddingOf(context).top, existingInset);
    expect(MediaQuery.viewPaddingOf(context).top, existingInset);
  });

  testWidgets('does not affect other desktop platforms', (tester) async {
    debugMacosWindowTargetPlatformOverride = TargetPlatform.linux;

    await tester.pumpWidget(
      const MediaQuery(
        data: MediaQueryData(),
        child: MacosWindowFrame(child: SizedBox(key: Key('content'))),
      ),
    );

    final context = tester.element(find.byKey(const Key('content')));
    expect(MediaQuery.paddingOf(context), EdgeInsets.zero);
    expect(find.byKey(const Key('macos-titlebar-safe-area')), findsNothing);
  });

  test('runner enables full-size content without hiding native buttons', () {
    final source = File(
      'macos/Runner/MainFlutterWindow.swift',
    ).readAsStringSync();

    expect(source, contains('styleMask.insert(.fullSizeContentView)'));
    expect(source, contains('titleVisibility = .hidden'));
    expect(source, contains('titlebarAppearsTransparent = true'));
    expect(source, isNot(contains('standardWindowButton')));
    expect(source, isNot(contains('styleMask = .borderless')));
  });
}
