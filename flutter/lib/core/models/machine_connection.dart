/// A configured daemon machine the app can connect to.
///
/// The [authKey] carries the long-term daemon key. It is persisted outside
/// this model — in the platform secure store (see `SecureKeyValueStore`) —
/// keyed by the machine [id]. The [toJson] output therefore never contains
/// the key; [toString] omits it as well. [tryFromJson] still tolerates a
/// legacy `auth_key` field so older plaintext SharedPreferences snapshots
/// can be migrated once into secure storage.
class MachineConnection {
  const MachineConnection({
    required this.id,
    required this.name,
    required this.baseUrl,
    required this.authKey,
    this.allowUntrustedCerts = false,
  });

  /// Stable unique id (uuid), used as the active-machine reference and as the
  /// secure-storage key namespace for [authKey].
  final String id;

  /// User-assigned label. May be empty; see [displayName].
  final String name;

  /// Daemon base URL, e.g. `http://192.0.2.10:8787`.
  final String baseUrl;

  /// Long-term daemon auth key. Held in memory only; persisted via
  /// `SecureKeyValueStore`, never via [toJson].
  final String authKey;

  /// Allow self-signed / untrusted TLS certificates for this machine.
  final bool allowUntrustedCerts;

  /// Human-facing label: the user [name] when set, otherwise `host:port`
  /// derived from [baseUrl], falling back to the raw URL.
  String get displayName {
    final trimmed = name.trim();
    if (trimmed.isNotEmpty) return trimmed;
    final uri = Uri.tryParse(baseUrl.trim());
    if (uri != null && uri.host.isNotEmpty) {
      return uri.hasPort ? '${uri.host}:${uri.port}' : uri.host;
    }
    final url = baseUrl.trim();
    return url.isNotEmpty ? url : id;
  }

  bool get isConfigured =>
      baseUrl.trim().isNotEmpty && authKey.trim().isNotEmpty;

  MachineConnection copyWith({
    String? name,
    String? baseUrl,
    String? authKey,
    bool? allowUntrustedCerts,
  }) {
    return MachineConnection(
      id: id,
      name: name ?? this.name,
      baseUrl: baseUrl ?? this.baseUrl,
      authKey: authKey ?? this.authKey,
      allowUntrustedCerts: allowUntrustedCerts ?? this.allowUntrustedCerts,
    );
  }

  /// Persists the non-secret metadata only. The auth key is written
  /// separately via `SecureKeyValueStore`; it must never appear here.
  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'base_url': baseUrl,
    'allow_untrusted_certs': allowUntrustedCerts,
  };

  /// Tolerant parser: unknown fields are ignored, missing fields fall back
  /// to defaults. Returns `null` when the entry has no usable id.
  ///
  /// For migration compatibility this still reads a legacy `auth_key` field
  /// (older versions stored it in plaintext SharedPreferences). Callers
  /// should move any such value into secure storage and rewrite the JSON
  /// without it; see `AppSettings._loadMachines`.
  static MachineConnection? tryFromJson(Object? decoded) {
    if (decoded is! Map<String, Object?>) return null;
    final id = decoded['id'];
    if (id is! String || id.trim().isEmpty) return null;
    return MachineConnection(
      id: id.trim(),
      name: decoded['name'] is String ? decoded['name'] as String : '',
      baseUrl: decoded['base_url'] is String
          ? decoded['base_url'] as String
          : '',
      authKey: decoded['auth_key'] is String
          ? decoded['auth_key'] as String
          : '',
      allowUntrustedCerts: decoded['allow_untrusted_certs'] == true,
    );
  }

  @override
  String toString() =>
      'MachineConnection(id: $id, name: $name, baseUrl: $baseUrl, '
      'allowUntrustedCerts: $allowUntrustedCerts)';
}
