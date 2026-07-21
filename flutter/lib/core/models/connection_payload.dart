import 'dart:convert';

/// Parsed contents of the connection QR code printed by phi-daemon at
/// startup (`{"type":"phi-daemon","version":1,"base_url":...,"auth_key":...}`).
///
/// The payload carries the long-term daemon key. It must never be written to
/// logs, error messages, snapshots, or test fixtures.
class ConnectionPayload {
  const ConnectionPayload({
    required this.baseUrl,
    required this.authKey,
    this.name = '',
  });

  /// Daemon base URL, e.g. `http://192.0.2.10:8787`. The scheme is `https`
  /// when the daemon serves TLS.
  final String baseUrl;

  /// Long-term daemon auth key (contents of PHI_DAEMON_AUTH_KEY_FILE).
  final String authKey;

  /// Optional human label for the machine (empty when the QR code does not
  /// carry one).
  final String name;

  static const String _expectedType = 'phi-daemon';
  static const int _expectedVersion = 1;

  /// Parses [raw] scanned QR text. Returns `null` for anything that is not a
  /// valid phi-daemon connection payload. Never throws and never includes the
  /// raw input (which may contain the key) in any error output.
  static ConnectionPayload? tryParse(String raw) {
    final Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } on FormatException {
      return null;
    }
    if (decoded is! Map<String, Object?>) return null;
    if (decoded['type'] != _expectedType) return null;
    if (decoded['version'] != _expectedVersion) return null;

    final baseUrl = decoded['base_url'];
    if (baseUrl is! String) return null;
    final uri = Uri.tryParse(baseUrl.trim());
    if (uri == null ||
        !uri.hasAuthority ||
        uri.host.isEmpty ||
        (uri.scheme != 'http' && uri.scheme != 'https')) {
      return null;
    }

    final authKey = decoded['auth_key'];
    if (authKey is! String || authKey.trim().isEmpty) return null;

    final name = decoded['name'];
    return ConnectionPayload(
      baseUrl: baseUrl.trim(),
      authKey: authKey.trim(),
      name: name is String ? name.trim() : '',
    );
  }
}
