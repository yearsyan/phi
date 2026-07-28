import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/ui/theme.dart';

void main() {
  testWidgets('desktop routes use a quick non-scaling transition', (
    tester,
  ) async {
    late BuildContext routeContext;
    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.light(),
        home: Builder(
          builder: (context) {
            routeContext = context;
            return const SizedBox();
          },
        ),
      ),
    );

    final builders = Theme.of(routeContext).pageTransitionsTheme.builders;
    final windowsBuilder = builders[TargetPlatform.windows];
    final macosBuilder = builders[TargetPlatform.macOS];
    expect(windowsBuilder, isA<DesktopPageTransitionsBuilder>());
    expect(macosBuilder, isA<DesktopPageTransitionsBuilder>());
    for (final builder in [windowsBuilder, macosBuilder]) {
      expect(builder!.transitionDuration, const Duration(milliseconds: 140));
      expect(
        builder.reverseTransitionDuration,
        const Duration(milliseconds: 100),
      );
    }

    final transition = macosBuilder!.buildTransitions<void>(
      MaterialPageRoute<void>(builder: (_) => const SizedBox()),
      routeContext,
      const AlwaysStoppedAnimation<double>(0.5),
      const AlwaysStoppedAnimation<double>(0),
      const SizedBox(key: Key('page')),
    );

    expect(transition, isA<FadeTransition>());
    final fade = transition as FadeTransition;
    expect(fade.child, isA<SlideTransition>());
    expect(
      (fade.child! as SlideTransition).child,
      isNot(isA<ScaleTransition>()),
    );
  });
}
