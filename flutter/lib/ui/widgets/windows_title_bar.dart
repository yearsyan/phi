import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../i18n/strings.dart';
import '../../platform/windows_window_controller.dart';

/// Installs the Flutter-rendered title bar only for the Windows runner.
class WindowsWindowFrame extends StatelessWidget {
  const WindowsWindowFrame({super.key, required this.child, this.controller});

  final Widget child;
  final WindowController? controller;

  @override
  Widget build(BuildContext context) {
    if (!windowsWindowControlsSupported) return child;
    final theme = Theme.of(context);
    final devicePixelRatio = MediaQuery.devicePixelRatioOf(context);
    final outlineColor = theme.brightness == Brightness.dark
        ? Colors.white.withAlpha(48)
        : Colors.black.withAlpha(38);
    return DecoratedBox(
      key: const Key('windows-window-outline'),
      position: DecorationPosition.foreground,
      decoration: BoxDecoration(
        border: Border.all(color: outlineColor, width: 1 / devicePixelRatio),
      ),
      child: Column(
        children: [
          WindowsTitleBar(controller: controller ?? WindowsWindowController()),
          Expanded(child: child),
        ],
      ),
    );
  }
}

class WindowsTitleBar extends StatefulWidget {
  const WindowsTitleBar({super.key, required this.controller});

  static const height = 32.0;
  static const defaultCaptionButtonWidth = 138.0;

  final WindowController controller;

  @override
  State<WindowsTitleBar> createState() => _WindowsTitleBarState();
}

class _WindowsTitleBarState extends State<WindowsTitleBar>
    with WidgetsBindingObserver {
  double _captionButtonWidth = WindowsTitleBar.defaultCaptionButtonWidth;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(_refreshCaptionButtonWidth());
  }

  @override
  void didUpdateWidget(WindowsTitleBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(widget.controller, oldWidget.controller)) {
      unawaited(_refreshCaptionButtonWidth());
    }
  }

  @override
  void didChangeMetrics() {
    unawaited(_refreshCaptionButtonWidth());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  Future<void> _refreshCaptionButtonWidth() async {
    try {
      final width = await widget.controller.captionButtonWidth();
      if (mounted && width > 0 && width != _captionButtonWidth) {
        setState(() => _captionButtonWidth = width);
      }
    } on MissingPluginException {
      // Widget tests and non-Windows embedders do not install this channel.
    } on PlatformException {
      // Keep the title bar usable if the native window is being torn down.
    }
  }

  Future<void> _toggleMaximize() async {
    try {
      await widget.controller.toggleMaximize();
    } on MissingPluginException {
      // See [_refreshCaptionButtonWidth].
    } on PlatformException {
      // The window may already be closing.
    }
  }

  Future<void> _startDragging() async {
    try {
      await widget.controller.startDragging();
    } on MissingPluginException {
      // See [_refreshCaptionButtonWidth].
    } on PlatformException {
      // Ignore a drag that races with native window teardown.
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final strings = S.of(context);

    return Material(
      key: const Key('windows-title-bar-surface'),
      color: colors.surfaceContainerHigh,
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(color: colors.outlineVariant, width: 0.5),
          ),
        ),
        child: SizedBox(
          height: WindowsTitleBar.height,
          child: Row(
            children: [
              Expanded(
                child: GestureDetector(
                  key: const Key('windows-title-drag-area'),
                  behavior: HitTestBehavior.opaque,
                  onPanStart: (_) => unawaited(_startDragging()),
                  onDoubleTap: () => unawaited(_toggleMaximize()),
                  child: Padding(
                    padding: const EdgeInsetsDirectional.only(start: 12),
                    child: Row(
                      children: [
                        Icon(
                          Icons.terminal_rounded,
                          size: 18,
                          color: colors.onSurface,
                        ),
                        const SizedBox(width: 8),
                        Text(
                          strings.appTitle,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.labelLarge?.copyWith(
                            fontWeight: FontWeight.w400,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
              SizedBox(
                key: const Key('windows-native-caption-button-region'),
                width: _captionButtonWidth,
              ),
            ],
          ),
        ),
      ),
    );
  }
}
