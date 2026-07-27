import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('macOS uses Keychain without provisioning-only entitlements', () {
    final adapterSource = File(
      'lib/platform/secure_storage.dart',
    ).readAsStringSync();

    expect(
      adapterSource,
      contains('MacOsOptions(useDataProtectionKeyChain: false)'),
      reason: 'ad-hoc macOS builds need the legacy system Keychain',
    );

    for (final path in [
      'macos/Runner/DebugProfile.entitlements',
      'macos/Runner/Release.entitlements',
    ]) {
      final source = File(path).readAsStringSync();

      expect(
        source,
        isNot(contains('<key>keychain-access-groups</key>')),
        reason: '$path must remain compatible with Flutter ad-hoc signing',
      );
    }
  });
}
