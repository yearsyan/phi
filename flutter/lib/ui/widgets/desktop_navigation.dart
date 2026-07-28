import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// Test hook for exercising desktop keyboard navigation on any host platform.
@visibleForTesting
TargetPlatform? debugDesktopNavigationTargetPlatformOverride;

bool get _usesDesktopKeyboardNavigation =>
    !kIsWeb &&
    switch (debugDesktopNavigationTargetPlatformOverride ??
        defaultTargetPlatform) {
      TargetPlatform.macOS || TargetPlatform.windows => true,
      _ => false,
    };

/// Restores the conventional Escape-to-dismiss shortcut inside MaterialApp's
/// builder.
///
/// This sits below Flutter's default text-editing shortcuts so Escape still
/// reaches modal routes on macOS when a text field has focus. Modal routes,
/// drawers, and menus retain ownership of the actual dismiss action.
class DesktopDismissShortcuts extends StatelessWidget {
  const DesktopDismissShortcuts({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!_usesDesktopKeyboardNavigation) return child;
    return Shortcuts(
      shortcuts: const <ShortcutActivator, Intent>{
        SingleActivator(LogicalKeyboardKey.escape): DismissIntent(),
      },
      child: child,
    );
  }
}

/// Lets a regular page opt into Escape-as-back on desktop without changing the
/// behavior of every pushed route in the application.
class DesktopRouteDismissRegion extends StatelessWidget {
  const DesktopRouteDismissRegion({
    super.key,
    required this.child,
    this.enabled = true,
  });

  final Widget child;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    if (!enabled || !_usesDesktopKeyboardNavigation) return child;
    return Actions(
      actions: <Type, Action<Intent>>{
        DismissIntent: CallbackAction<DismissIntent>(
          onInvoke: (_) => Navigator.maybeOf(context)?.maybePop(),
        ),
      },
      child: Focus(autofocus: true, child: child),
    );
  }
}
