import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/platform/image_attachment_picker.dart';
import 'package:phi_client/state/daemon_client.dart';
import 'package:phi_client/state/session_controller.dart';
import 'package:phi_client/ui/theme.dart';
import 'package:phi_client/ui/widgets/composer.dart';
import 'package:phi_client/ui/widgets/timeline.dart';

void main() {
  test('image attachments serialize as daemon multimodal content', () {
    final image = PickedImageAttachment(
      name: 'pixel.jpg',
      mimeType: 'image/jpeg',
      bytes: Uint8List.fromList([1, 2, 3]),
    );
    final content = Content.parts([
      const ContentPart.text('describe this'),
      ContentPart.imageUrl(image.dataUrl, detail: 'auto'),
    ]);

    expect(content.toJson(), {
      'type': 'parts',
      'value': [
        {'type': 'text', 'text': 'describe this'},
        {
          'type': 'image_url',
          'image_url': {'url': 'data:image/jpeg;base64,AQID', 'detail': 'auto'},
        },
      ],
    });
  });

  for (final width in <double>[320, 360, 390]) {
    testWidgets('composer fits ${width.toInt()}dp with full labels', (
      tester,
    ) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = Size(width, 800);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);

      final client = DaemonClient(_FakeTransport());
      final controller =
          SessionController(client: client, target: const NewSessionTarget())
            ..phase = SessionConnectionPhase.ready
            ..ready = true
            ..capabilityMode = CapabilityMode.workspaceEdit
            ..config = const SessionConfig(
              model: 'deepseek-v4-pro',
              reasoningEffort: 'max',
              revision: 1,
            );
      addTearDown(controller.dispose);

      await tester.pumpWidget(
        MaterialApp(
          locale: const Locale('zh'),
          supportedLocales: const [Locale('zh')],
          localizationsDelegates: GlobalMaterialLocalizations.delegates,
          theme: AppTheme.light(),
          home: Scaffold(
            body: Column(
              children: [
                const Expanded(child: SizedBox()),
                Composer(controller: controller, client: client),
              ],
            ),
          ),
        ),
      );

      expect(find.text('最高'), findsOneWidget);
      expect(find.text('deepseek-v4-pro'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('composer-model-reasoning-button')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('composer-capability-button')),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('composer-add-button')), findsOneWidget);
      final emptySendButton = tester.widget<IconButton>(
        find.descendant(
          of: find.byKey(const ValueKey('composer-send-button')),
          matching: find.byType(IconButton),
        ),
      );
      expect(emptySendButton.onPressed, isNull);
      expect(emptySendButton.icon, isA<Icon>());
      expect(tester.takeException(), isNull);

      await tester.tap(
        find.byKey(const ValueKey('composer-model-reasoning-button')),
      );
      await tester.pumpAndSettle();

      expect(find.text('模型与推理'), findsOneWidget);
      expect(find.text('模型'), findsOneWidget);
      expect(find.text('推理强度'), findsOneWidget);
      expect(find.text('deepseek-v4-pro'), findsNWidgets(2));
      expect(find.text('最高'), findsNWidgets(2));
      expect(tester.takeException(), isNull);

      tester.state<NavigatorState>(find.byType(Navigator).first).pop();
      await tester.pumpAndSettle();

      await tester.enterText(
        find.byKey(const ValueKey('composer-text-field')),
        'A long draft that wraps onto multiple lines on a narrow phone.',
      );
      await tester.pump();

      expect(
        find.byKey(const ValueKey('composer-send-button')),
        findsOneWidget,
      );
      final readySendButton = tester.widget<IconButton>(
        find.descendant(
          of: find.byKey(const ValueKey('composer-send-button')),
          matching: find.byType(IconButton),
        ),
      );
      expect(readySendButton.onPressed, isNotNull);
      expect(find.text('最高'), findsOneWidget);
      expect(find.text('deepseek-v4-pro'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('timeline dissolves into a gradient above the composer', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    final client = DaemonClient(_FakeTransport());
    final controller =
        SessionController(client: client, target: const NewSessionTarget())
          ..phase = SessionConnectionPhase.ready
          ..ready = true
          ..history = [
            const PublicMessage(role: 'user', content: Content.text('hi')),
            const PublicMessage(
              role: 'assistant',
              content: Content.text('hello'),
            ),
          ];
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: Scaffold(
          body: ChatTimeline(
            controller: controller,
            scrollController: ScrollController(),
            onFork: (_) {},
          ),
        ),
      ),
    );

    final fade = find.byKey(const ValueKey('timeline-bottom-fade'));
    expect(fade, findsOneWidget);
    expect(
      tester
          .widget<IgnorePointer>(
            find.ancestor(of: fade, matching: find.byType(IgnorePointer)).first,
          )
          .ignoring,
      isTrue,
    );

    final theme = AppTheme.light();
    final decoration =
        tester
                .widget<DecoratedBox>(
                  find.descendant(
                    of: fade,
                    matching: find.byType(DecoratedBox),
                  ),
                )
                .decoration
            as BoxDecoration;
    final gradient = decoration.gradient! as LinearGradient;
    expect(gradient.begin, Alignment.topCenter);
    expect(gradient.end, Alignment.bottomCenter);
    expect(gradient.colors.first.a, 0);
    expect(gradient.colors.last, theme.scaffoldBackgroundColor);
    expect(tester.takeException(), isNull);
  });

  testWidgets('lone streaming thinking header stays left-aligned', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    final client = DaemonClient(_FakeTransport());
    final controller =
        SessionController(client: client, target: const NewSessionTarget())
          ..phase = SessionConnectionPhase.ready
          ..ready = true
          ..history = [
            const PublicMessage(role: 'user', content: Content.text('hi')),
          ]
          ..draft = const AssistantDraft(reasoning: ' pondering');
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: Scaffold(
          body: ChatTimeline(
            controller: controller,
            scrollController: ScrollController(),
            onFork: (_) {},
          ),
        ),
      ),
    );

    final header = find.text('思考中…');
    expect(header, findsOneWidget);
    final headerLeft = tester.getTopLeft(header).dx;
    final listLeft = tester.getTopLeft(find.byType(ListView)).dx;
    // 12dp list padding + 16dp chevron + 4dp gap; centred would be ~135dp.
    expect(headerLeft - listLeft, lessThan(40));
    expect(tester.takeException(), isNull);
  });

  testWidgets('completed run does not repeat its tools after the answer', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const call = ToolCall(id: 'call-1', name: 'read', arguments: {'path': 'x'});
    final client = DaemonClient(_FakeTransport());
    final controller =
        SessionController(client: client, target: const NewSessionTarget())
          ..phase = SessionConnectionPhase.ready
          ..ready = true
          ..history = [
            const PublicMessage(role: 'user', content: Content.text('explore')),
            const PublicMessage(role: 'assistant', toolCalls: [call]),
            const PublicMessage(
              role: 'tool',
              toolCallId: 'call-1',
              content: Content.text('file body'),
            ),
            const PublicMessage(
              role: 'assistant',
              content: Content.text('final answer'),
            ),
          ]
          ..activeRun = (RunActivity(runId: 'run-1', historyStart: 1)
            ..status = 'completed'
            ..turns.add(
              TurnActivity(1)
                ..finished = true
                ..steps.add(
                  ToolStep(call: call)
                    ..done = true
                    ..content = 'file body',
                ),
            ));
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        locale: const Locale('zh'),
        supportedLocales: const [Locale('zh')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: AppTheme.light(),
        home: Scaffold(
          body: ChatTimeline(
            controller: controller,
            scrollController: ScrollController(),
            onFork: (_) {},
          ),
        ),
      ),
    );

    expect(find.text('final answer'), findsOneWidget);
    // The committed call renders exactly once, inline with its message —
    // no duplicate tile or trailing per-turn summary after the answer.
    expect(find.text('read'), findsOneWidget);
    expect(find.textContaining('使用了'), findsNothing);
    expect(find.text('运行完成'), findsOneWidget);
    // The run-completion status is the only entry below the final answer.
    final answerBottom = tester.getBottomLeft(find.text('final answer')).dy;
    final statusTop = tester.getTopLeft(find.text('运行完成')).dy;
    final tileTop = tester.getTopLeft(find.text('read')).dy;
    expect(statusTop, greaterThan(answerBottom));
    expect(tileTop, lessThan(answerBottom));
    expect(tester.takeException(), isNull);
  });
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
