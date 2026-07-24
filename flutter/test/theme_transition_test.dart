import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/ui/theme.dart';

void main() {
  testWidgets('Windows routes use a compact non-scaling transition', (
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

    final builder = Theme.of(
      routeContext,
    ).pageTransitionsTheme.builders[TargetPlatform.windows];
    expect(builder, isA<WindowsPageTransitionsBuilder>());
    expect(
      builder!.transitionDuration,
      WindowsPageTransitionsBuilder.forwardDuration,
    );
    expect(
      builder.reverseTransitionDuration,
      WindowsPageTransitionsBuilder.backwardDuration,
    );

    final transition = builder.buildTransitions<void>(
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
