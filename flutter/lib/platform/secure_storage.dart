import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Stores small secret strings (daemon auth keys) outside plaintext
/// SharedPreferences.
///
/// Production uses each platform's native credential store, delegating to
/// `flutter_secure_storage` except for the prompt-free macOS adapter below:
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
  if (!kIsWeb && defaultTargetPlatform == TargetPlatform.macOS) {
    return const MacOsPromptFreeKeychainStore();
  }
  return const _FlutterSecureStorageAdapter();
}

/// macOS Keychain adapter whose native implementation never permits
/// SecurityAgent to display an authentication dialog.
///
/// Release builds use a stable Developer ID identity, but legacy items may
/// still have an ACL created by an older ad-hoc build. The native side reads
/// those items only when macOS can authorize the access without interaction,
/// then migrates them into Phi's current service namespace.
@visibleForTesting
class MacOsPromptFreeKeychainStore implements SecureKeyValueStore {
  const MacOsPromptFreeKeychainStore({
    MethodChannel channel = const MethodChannel(
      'dev.phi.phi_client/prompt_free_keychain',
    ),
  }) : _channel = channel;

  final MethodChannel _channel;

  @override
  Future<String?> read(String key) =>
      _channel.invokeMethod<String>('read', <String, Object?>{'key': key});

  @override
  Future<void> write(String key, String value) => _channel.invokeMethod<void>(
    'write',
    <String, Object?>{'key': key, 'value': value},
  );

  @override
  Future<void> delete(String key) =>
      _channel.invokeMethod<void>('delete', <String, Object?>{'key': key});
}

class _FlutterSecureStorageAdapter implements SecureKeyValueStore {
  const _FlutterSecureStorageAdapter();

  static const _storage = FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
  );

  @override
  Future<String?> read(String key) => _storage.read(key: key);

  @override
  Future<void> write(String key, String value) =>
      _storage.write(key: key, value: value);

  @override
  Future<void> delete(String key) => _storage.delete(key: key);
}
