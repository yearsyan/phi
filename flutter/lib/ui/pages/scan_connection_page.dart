import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../../core/models/connection_payload.dart';
import '../../i18n/strings.dart';

/// Full-screen camera page that scans the connection QR code printed by
/// phi-daemon at startup. Pops with the parsed [ConnectionPayload] on success,
/// or with `null` when the user leaves without scanning.
class ScanConnectionPage extends StatefulWidget {
  const ScanConnectionPage({super.key});

  @override
  State<ScanConnectionPage> createState() => _ScanConnectionPageState();
}

class _ScanConnectionPageState extends State<ScanConnectionPage> {
  bool _handled = false;
  DateTime _lastInvalidNotice = DateTime.fromMillisecondsSinceEpoch(0);

  void _onDetect(BarcodeCapture capture) {
    if (_handled) return;
    for (final barcode in capture.barcodes) {
      final raw = barcode.rawValue;
      if (raw == null) continue;
      final payload = ConnectionPayload.tryParse(raw);
      if (payload == null) {
        _notifyInvalid();
        continue;
      }
      _handled = true;
      Navigator.of(context).pop(payload);
      return;
    }
  }

  void _notifyInvalid() {
    // Detections fire per frame; throttle the snackbar so an unrelated QR
    // held in view does not spam notifications.
    final now = DateTime.now();
    if (now.difference(_lastInvalidNotice) < const Duration(seconds: 2)) {
      return;
    }
    _lastInvalidNotice = now;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(S.of(context).invalidQrCode)));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: Text(S.of(context).scanQrCode)),
      body: Stack(
        fit: StackFit.expand,
        children: [
          MobileScanner(
            onDetect: _onDetect,
            errorBuilder: (context, error) => _ScannerError(error: error),
          ),
          Center(
            child: Container(
              width: 240,
              height: 240,
              decoration: BoxDecoration(
                border: Border.all(color: theme.colorScheme.primary, width: 2),
                borderRadius: BorderRadius.circular(12),
              ),
            ),
          ),
          Positioned(
            left: 24,
            right: 24,
            bottom: 48,
            child: Text(
              S.of(context).scanQrHint,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: Colors.white,
                shadows: const [Shadow(blurRadius: 8)],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ScannerError extends StatelessWidget {
  const _ScannerError({required this.error});

  final MobileScannerException error;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    final message = error.errorCode == MobileScannerErrorCode.permissionDenied
        ? s.cameraPermissionDenied
        : s.cameraUnavailable;
    return ColoredBox(
      color: theme.colorScheme.surface,
      child: Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                Icons.no_photography_outlined,
                size: 40,
                color: theme.colorScheme.outline,
              ),
              const SizedBox(height: 12),
              Text(
                message,
                textAlign: TextAlign.center,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
