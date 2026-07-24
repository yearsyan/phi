import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

const _windowControlsChannelName = 'dev.phi.phi_client/window_controls';

/// Whether the Flutter-rendered Windows title bar should be installed.
bool get windowsWindowControlsSupported {
  final override = debugWindowsWindowControlsSupportedOverride;
  if (override != null) return override;
  if (kIsWeb) return false;
  return Platform.isWindows;
}

/// Test hook to force Windows window controls regardless of the host platform.
@visibleForTesting
bool? debugWindowsWindowControlsSupportedOverride;

/// Operations supplied by the native top-level window.
abstract interface class WindowController {
  Future<double> captionButtonWidth();

  Future<bool> isMaximized();

  Future<void> minimize();

  Future<bool> toggleMaximize();

  Future<void> startDragging();

  Future<void> close();
}

/// Method-channel adapter for the Windows runner's top-level window.
class WindowsWindowController implements WindowController {
  WindowsWindowController({MethodChannel? channel})
    : _channel = channel ?? const MethodChannel(_windowControlsChannelName);

  final MethodChannel _channel;

  @override
  Future<double> captionButtonWidth() async =>
      await _channel.invokeMethod<double>('captionButtonWidth') ?? 138;

  @override
  Future<bool> isMaximized() async =>
      await _channel.invokeMethod<bool>('isMaximized') ?? false;

  @override
  Future<void> minimize() => _channel.invokeMethod<void>('minimize');

  @override
  Future<bool> toggleMaximize() async =>
      await _channel.invokeMethod<bool>('toggleMaximize') ?? false;

  @override
  Future<void> startDragging() => _channel.invokeMethod<void>('startDragging');

  @override
  Future<void> close() => _channel.invokeMethod<void>('close');
}
