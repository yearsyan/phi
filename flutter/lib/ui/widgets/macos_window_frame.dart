import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

/// Test hook for exercising macOS window chrome on other host platforms.
@visibleForTesting
TargetPlatform? debugMacosWindowTargetPlatformOverride;

/// Keeps Flutter controls clear of the native macOS traffic-light buttons.
///
/// The macOS runner uses a full-size content view, so Flutter paints all the
/// way to the top window edge. Desktop Flutter does not expose the AppKit title
/// bar as a safe-area inset, so inject the standard compact-title-bar height.
/// Backgrounds still paint behind this inset; widgets such as [AppBar] consume
/// it and place interactive content below the native controls.
class MacosWindowFrame extends StatelessWidget {
  const MacosWindowFrame({super.key, required this.child});

  static const titleBarSafeInset = 28.0;

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final targetPlatform =
        debugMacosWindowTargetPlatformOverride ?? defaultTargetPlatform;
    if (kIsWeb || targetPlatform != TargetPlatform.macOS) {
      return child;
    }

    final mediaQuery = MediaQuery.of(context);
    final padding = mediaQuery.padding;
    final viewPadding = mediaQuery.viewPadding;
    return MediaQuery(
      key: const Key('macos-titlebar-safe-area'),
      data: mediaQuery.copyWith(
        padding: padding.copyWith(
          top: math.max(padding.top, titleBarSafeInset),
        ),
        viewPadding: viewPadding.copyWith(
          top: math.max(viewPadding.top, titleBarSafeInset),
        ),
      ),
      child: child,
    );
  }
}
