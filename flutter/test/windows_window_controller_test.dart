import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/platform/windows_window_controller.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('dev.phi.phi_client/window_controls');

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test(
    'Windows controller maps every window operation to the native channel',
    () async {
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call);
            return switch (call.method) {
              'captionButtonWidth' => 138.0,
              'isMaximized' => false,
              'toggleMaximize' => true,
              _ => null,
            };
          });

      final controller = WindowsWindowController(channel: channel);

      expect(await controller.captionButtonWidth(), 138);
      expect(await controller.isMaximized(), isFalse);
      await controller.minimize();
      expect(await controller.toggleMaximize(), isTrue);
      await controller.startDragging();
      await controller.close();

      expect(calls.map((call) => call.method), [
        'captionButtonWidth',
        'isMaximized',
        'minimize',
        'toggleMaximize',
        'startDragging',
        'close',
      ]);
    },
  );

  test('missing native maximized values default to false', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (_) async => null);

    final controller = WindowsWindowController(channel: channel);

    expect(await controller.captionButtonWidth(), 138);
    expect(await controller.isMaximized(), isFalse);
    expect(await controller.toggleMaximize(), isFalse);
  });
}
