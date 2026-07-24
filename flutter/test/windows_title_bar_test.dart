import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/platform/windows_window_controller.dart';
import 'package:phi_client/ui/widgets/windows_title_bar.dart';

class _FakeWindowController implements WindowController {
  double nativeCaptionButtonWidth = 138;
  int captionButtonWidthCalls = 0;
  bool maximized = false;
  int minimizeCalls = 0;
  int toggleMaximizeCalls = 0;
  int startDraggingCalls = 0;
  int closeCalls = 0;

  @override
  Future<double> captionButtonWidth() async {
    captionButtonWidthCalls += 1;
    return nativeCaptionButtonWidth;
  }

  @override
  Future<void> close() async {
    closeCalls += 1;
  }

  @override
  Future<bool> isMaximized() async => maximized;

  @override
  Future<void> minimize() async {
    minimizeCalls += 1;
  }

  @override
  Future<void> startDragging() async {
    startDraggingCalls += 1;
  }

  @override
  Future<bool> toggleMaximize() async {
    toggleMaximizeCalls += 1;
    maximized = !maximized;
    return maximized;
  }
}

Future<void> _pumpFrame(
  WidgetTester tester,
  _FakeWindowController controller,
) async {
  await tester.pumpWidget(
    MaterialApp(
      locale: const Locale('zh'),
      supportedLocales: const [Locale('zh')],
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      home: WindowsWindowFrame(
        controller: controller,
        child: const Scaffold(body: Text('content')),
      ),
    ),
  );
  await tester.pump();
}

void main() {
  setUp(() {
    debugWindowsWindowControlsSupportedOverride = true;
  });

  tearDown(() {
    debugWindowsWindowControlsSupportedOverride = null;
  });

  testWidgets('title bar reserves the native caption button overlay', (
    tester,
  ) async {
    final controller = _FakeWindowController();
    await _pumpFrame(tester, controller);

    expect(find.byType(WindowsTitleBar), findsOneWidget);
    final nativeRegion = tester.widget<SizedBox>(
      find.byKey(const Key('windows-native-caption-button-region')),
    );
    expect(nativeRegion.width, controller.nativeCaptionButtonWidth);
    expect(find.byKey(const Key('windows-minimize-button')), findsNothing);
    expect(find.byKey(const Key('windows-maximize-button')), findsNothing);
    expect(find.byKey(const Key('windows-close-button')), findsNothing);
    expect(controller.captionButtonWidthCalls, 1);
    expect(controller.minimizeCalls, 0);
    expect(controller.toggleMaximizeCalls, 0);
    expect(controller.closeCalls, 0);
  });

  testWidgets('drag area starts a native window move', (tester) async {
    final controller = _FakeWindowController();
    await _pumpFrame(tester, controller);

    await tester.drag(
      find.byKey(const Key('windows-title-drag-area')),
      const Offset(30, 0),
    );
    await tester.pump(const Duration(milliseconds: 100));

    expect(controller.startDraggingCalls, 1);
  });

  testWidgets('double-clicking the app title toggles native maximize', (
    tester,
  ) async {
    final controller = _FakeWindowController();
    await _pumpFrame(tester, controller);
    final dragArea = find.byKey(const Key('windows-title-drag-area'));

    await tester.tap(dragArea);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(dragArea);
    await tester.pump(const Duration(milliseconds: 100));

    expect(controller.toggleMaximizeCalls, 1);
  });

  testWidgets('window frame paints a subtle physical-pixel outline', (
    tester,
  ) async {
    final controller = _FakeWindowController();
    await _pumpFrame(tester, controller);

    final outlineFinder = find.byKey(const Key('windows-window-outline'));
    final outline = tester.widget<DecoratedBox>(outlineFinder);
    final decoration = outline.decoration as BoxDecoration;
    final border = decoration.border! as Border;
    final devicePixelRatio = MediaQuery.devicePixelRatioOf(
      tester.element(outlineFinder),
    );

    expect(outline.position, DecorationPosition.foreground);
    expect(border.top.width, 1 / devicePixelRatio);

    final titleSurface = tester.widget<Material>(
      find.byKey(const Key('windows-title-bar-surface')),
    );
    final colors = Theme.of(
      tester.element(find.byType(WindowsTitleBar)),
    ).colorScheme;
    expect(titleSurface.color, colors.surfaceContainerHigh);
  });

  testWidgets('frame is transparent when Windows controls are disabled', (
    tester,
  ) async {
    debugWindowsWindowControlsSupportedOverride = false;
    final controller = _FakeWindowController();
    await _pumpFrame(tester, controller);

    expect(find.byType(WindowsTitleBar), findsNothing);
    expect(find.text('content'), findsOneWidget);
  });
}
