/// A configured daemon machine the app can connect to.
///
/// The [authKey] carries the long-term daemon key. It must never be written
/// to logs, error messages, snapshots, or test fixtures; [toString] and
/// [toJson] callers must treat the JSON output as sensitive.
class MachineConnection {
  const MachineConnection({
    required this.id,
    required this.name,
    required this.baseUrl,
    required this.authKey,
    this.allowUntrustedCerts = false,
  });

  /// Stable unique id (uuid), used as the active-machine reference.
  final String id;

  /// User-assigned label. May be empty; see [displayName].
  final String name;

  /// Daemon base URL, e.g. `http://192.0.2.10:8787`.
  final String baseUrl;

  /// Long-term daemon auth key.
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

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'base_url': baseUrl,
    'auth_key': authKey,
    'allow_untrusted_certs': allowUntrustedCerts,
  };

  /// Tolerant parser: unknown fields are ignored, missing fields fall back
  /// to defaults. Returns `null` when the entry has no usable id.
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
