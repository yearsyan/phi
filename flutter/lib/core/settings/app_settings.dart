import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../transport/daemon_transport.dart';
import '../transport/direct_transport.dart';

/// Kind of transport used to reach the daemon.
///
/// Only [direct] is implemented today; the enum exists so connection
/// settings can already express HTTP-over-SSH / HTTP-over-Tailscale
/// transports once they land (they will plug in as [DaemonTransport]
/// implementations).
enum TransportKind { direct, ssh, tailscale }

/// Persisted app settings: how to reach the daemon plus UI preferences.
class AppSettings extends ChangeNotifier {
  AppSettings._();

  static const _kBaseUrl = 'daemon.base_url';
  static const _kAuthKey = 'daemon.auth_key';
  static const _kAllowUntrustedCerts = 'daemon.allow_untrusted_certs';
  static const _kRecentWorkspaces = 'ui.recent_workspaces';
  static const _kDefaultCapabilityMode = 'ui.default_capability_mode';
  static const _kAppLanguage = 'ui.app_language';

  /// Optional build-time seed values (development / CI convenience):
  /// `--dart-define=PHI_DAEMON_URL=... --dart-define=PHI_DAEMON_KEY=...`
  static const _seedUrl = String.fromEnvironment('PHI_DAEMON_URL');
  static const _seedKey = String.fromEnvironment('PHI_DAEMON_KEY');

  static Future<AppSettings> load() async {
    final settings = AppSettings._();
    final prefs = await SharedPreferences.getInstance();
    settings._prefs = prefs;
    settings._baseUrl =
        prefs.getString(_kBaseUrl) ??
        (_seedUrl.isNotEmpty ? _seedUrl : 'http://127.0.0.1:8787');
    settings._authKey = prefs.getString(_kAuthKey) ?? _seedKey;
    settings._allowUntrustedCerts =
        prefs.getBool(_kAllowUntrustedCerts) ?? false;
    settings._recentWorkspaces =
        prefs.getStringList(_kRecentWorkspaces) ?? <String>[];
    settings._defaultCapabilityMode =
        prefs.getString(_kDefaultCapabilityMode) ?? 'workspace_edit';
    settings._appLanguage = prefs.getString(_kAppLanguage) ?? 'system';
    return settings;
  }

  late final SharedPreferences _prefs;

  String _baseUrl = 'http://127.0.0.1:8787';
  String _authKey = '';
  bool _allowUntrustedCerts = false;
  List<String> _recentWorkspaces = [];
  String _defaultCapabilityMode = 'workspace_edit';
  String _appLanguage = 'system'; // system | en | zh

  String get baseUrl => _baseUrl;
  String get authKey => _authKey;
  bool get allowUntrustedCerts => _allowUntrustedCerts;
  List<String> get recentWorkspaces => List.unmodifiable(_recentWorkspaces);
  String get defaultCapabilityMode => _defaultCapabilityMode;

  /// Language override: `system`, `en` or `zh`.
  String get appLanguage => _appLanguage;

  bool get isConfigured =>
      _baseUrl.trim().isNotEmpty && _authKey.trim().isNotEmpty;

  DaemonTransport? _transport;

  /// The current transport. Rebuilt whenever connection settings change.
  DaemonTransport get transport {
    final existing = _transport;
    if (existing != null) return existing;
    final created = _buildTransport();
    _transport = created;
    return created;
  }

  DaemonTransport _buildTransport() {
    var url = _baseUrl.trim();
    if (url.isEmpty) url = 'http://127.0.0.1:8787';
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
      url = 'http://$url';
    }
    return DirectDaemonTransport(
      baseUri: Uri.parse(url),
      authKey: _authKey.trim(),
      allowUntrustedCerts: _allowUntrustedCerts,
    );
  }

  void _replaceTransport() {
    _transport?.dispose();
    _transport = null;
  }

  Future<void> updateConnection({
    required String baseUrl,
    required String authKey,
    bool? allowUntrustedCerts,
  }) async {
    _baseUrl = baseUrl.trim();
    _authKey = authKey.trim();
    if (allowUntrustedCerts != null) {
      _allowUntrustedCerts = allowUntrustedCerts;
    }
    await _prefs.setString(_kBaseUrl, _baseUrl);
    await _prefs.setString(_kAuthKey, _authKey);
    await _prefs.setBool(_kAllowUntrustedCerts, _allowUntrustedCerts);
    _replaceTransport();
    notifyListeners();
  }

  Future<void> setDefaultCapabilityMode(String mode) async {
    _defaultCapabilityMode = mode;
    await _prefs.setString(_kDefaultCapabilityMode, mode);
    notifyListeners();
  }

  Future<void> setAppLanguage(String language) async {
    _appLanguage = language;
    await _prefs.setString(_kAppLanguage, language);
    notifyListeners();
  }

  Future<void> addRecentWorkspace(String path) async {
    if (path.trim().isEmpty) return;
    _recentWorkspaces = [path, ..._recentWorkspaces.where((p) => p != path)];
    if (_recentWorkspaces.length > 12) {
      _recentWorkspaces = _recentWorkspaces.sublist(0, 12);
    }
    await _prefs.setStringList(_kRecentWorkspaces, _recentWorkspaces);
    notifyListeners();
  }
}
