import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('macOS Keychain access cannot display authentication UI', () {
    final adapterSource = File(
      'lib/platform/secure_storage.dart',
    ).readAsStringSync();
    final runnerSource = File(
      'macos/Runner/MainFlutterWindow.swift',
    ).readAsStringSync();

    expect(
      adapterSource,
      contains('return const MacOsPromptFreeKeychainStore()'),
      reason: 'macOS must use the native prompt-free Keychain adapter',
    );
    expect(
      runnerSource,
      contains('context.interactionNotAllowed = true'),
      reason: 'native Keychain queries must forbid authentication UI',
    );
    expect(
      runnerSource,
      contains('dev.phi.phiClient.daemon-auth.v2'),
      reason: 'old ad-hoc ACL entries must not be queried as current items',
    );
    expect(
      runnerSource,
      contains('dev.phi.phiClient.daemon-auth.local.v2'),
      reason: 'ad-hoc builds must not create items in the signed app namespace',
    );
    expect(
      runnerSource,
      contains('kSecCodeInfoTeamIdentifier'),
      reason: 'the runtime signature must select the Keychain namespace',
    );
    expect(
      runnerSource,
      contains('flutter_secure_storage_service'),
      reason: 'already-authorized legacy items should migrate noninteractively',
    );

    for (final path in [
      'macos/Runner/DebugProfile.entitlements',
      'macos/Runner/Release.entitlements',
    ]) {
      final source = File(path).readAsStringSync();

      expect(
        source,
        isNot(contains('<key>keychain-access-groups</key>')),
        reason: '$path must remain compatible with local ad-hoc builds',
      );
    }
  });
}
