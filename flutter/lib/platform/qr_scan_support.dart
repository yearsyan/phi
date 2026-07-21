import 'dart:io';

import 'package:flutter/foundation.dart';

/// Whether this device can show the "scan daemon QR code" entry points.
///
/// `mobile_scanner` supports Android/iOS/macOS/Web, but the scan flow only
/// makes sense on mobile (the daemon prints the QR on its own terminal), and
/// there is no OpenHarmony implementation, so the entries are mobile-only.
bool get qrScanSupported {
  final override = debugQrScanSupportedOverride;
  if (override != null) return override;
  if (kIsWeb) return false;
  return Platform.isAndroid || Platform.isIOS;
}

/// Test hook to force [qrScanSupported] regardless of the host platform.
@visibleForTesting
bool? debugQrScanSupportedOverride;
