import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Stores small secret strings (daemon auth keys) outside plaintext
/// SharedPreferences.
///
/// The production implementation delegates to `flutter_secure_storage`, which
/// uses each platform's native credential store:
/// - Android: EncryptedSharedPreferences (Jetpack Security, AES-GCM)
/// - iOS / macOS: Keychain
/// - Windows: DPAPI
/// - OpenHarmony: AES + RSA-wrapped key (via `flutter_secure_storage_ohos`)
///
/// `SharedPreferences` remains in use for non-sensitive UI prefs; only the
/// long-lived daemon auth key is routed through this layer. Callers always
/// receive the plaintext value so the transport layer is unchanged.
abstract interface class SecureKeyValueStore {
  /// Reads the value for [key], or `null` when absent.
  Future<String?> read(String key);

  /// Writes [value] for [key], overwriting any prior value.
  Future<void> write(String key, String value);

  /// Removes [key]. A missing key is not an error.
  Future<void> delete(String key);
}

/// Test hook: when non-null, [defaultSecureStorage] returns this instance
/// instead of the platform-backed one. Mirrors the `debug*Override` pattern
/// used by `qr_scan_support.dart` and `windows_window_controller.dart`.
@visibleForTesting
SecureKeyValueStore? debugSecureStorageOverride;

/// The process-wide secure store. Tests override via [debugSecureStorageOverride].
SecureKeyValueStore defaultSecureStorage() {
  final override = debugSecureStorageOverride;
  if (override != null) return override;
  return const _FlutterSecureStorageAdapter();
}

class _FlutterSecureStorageAdapter implements SecureKeyValueStore {
  const _FlutterSecureStorageAdapter();

  static const _storage = FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
    // Flutter's macOS runner is ad-hoc signed by default. The data-protection
    // Keychain requires a provisioned application identifier, so it fails
    // with errSecMissingEntitlement (-34018) in local desktop builds. The
    // legacy macOS Keychain remains encrypted and managed by Keychain
    // Services, while working with the runner's default signing setup.
    mOptions: MacOsOptions(useDataProtectionKeyChain: false),
  );

  @override
  Future<String?> read(String key) => _storage.read(key: key);

  @override
  Future<void> write(String key, String value) =>
      _storage.write(key: key, value: value);

  @override
  Future<void> delete(String key) => _storage.delete(key: key);
}
