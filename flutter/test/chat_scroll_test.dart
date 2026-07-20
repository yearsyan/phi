import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/app.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/app_state.dart';
import 'package:phi_client/state/daemon_client.dart';
import 'package:phi_client/state/session_controller.dart';
import 'package:phi_client/ui/pages/chat_page.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  testWidgets('user can scroll up while output streams', (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    SharedPreferences.setMockInitialValues({});
    final app = AppState(await AppSettings.load());
    addTearDown(app.dispose);

    late _TestSessionController controller;
    await tester.pumpWidget(
      MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: AppScope(
          state: app,
          child: ChatPage(
            target: const NewSessionTarget(),
            controllerFactory: () {
              controller = _TestSessionController(
                client: DaemonClient(_FakeTransport()),
                target: const NewSessionTarget(),
              );
              return controller;
            },
          ),
        ),
      ),
    );
    await tester.pump();

    controller
      ..phase = SessionConnectionPhase.ready
      ..ready = true
      ..status = SessionStatus.running
      ..activeRunId = 'run-1'
      ..history = [
        for (var i = 0; i < 30; i++)
          PublicMessage(
            role: 'user',
            content: Content.text('message $i with some extra text for height'),
          ),
      ]
      ..draft = const AssistantDraft(text: 'streaming');
    controller.poke();
    await tester.pump(); // ListView attaches
    controller.poke(); // schedules the stick-to-bottom jump
    await tester.pump(); // jump executes

    final position = tester
        .state<ScrollableState>(find.byType(Scrollable).first)
        .position;
    expect(position.pixels, closeTo(position.maxScrollExtent, 1));

    // Drag up in small steps while deltas keep arriving: each step used to be
    // cancelled by the stick-to-bottom jump, pinning the view in place.
    final gesture = await tester.startGesture(
      tester.getCenter(find.byType(ListView)),
    );
    for (var i = 0; i < 8; i++) {
      await gesture.moveBy(const Offset(0, 50));
      controller.draft = AssistantDraft(text: 'streaming ${'x' * (80 * i)}');
      controller.poke();
      await tester.pump();
    }
    await gesture.up();
    await tester.pump();

    expect(
      position.maxScrollExtent - position.pixels,
      greaterThan(200),
      reason: 'the user must gain reading distance during streaming',
    );
    expect(tester.takeException(), isNull);
  });
}

class _TestSessionController extends SessionController {
  _TestSessionController({required super.client, required super.target});

  @override
  void start() {} // no socket in tests

  void poke() => notifyListeners();
}

class _FakeTransport implements DaemonTransport {
  @override
  String get displayName => 'fake';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async => throw UnsupportedError('network is not used by this test');

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async => throw UnsupportedError('network is not used by this test');

  @override
  void dispose() {}
}
