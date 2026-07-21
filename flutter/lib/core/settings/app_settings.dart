import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:uuid/uuid.dart';

import '../models/machine_connection.dart';
import '../transport/daemon_transport.dart';
import '../transport/direct_transport.dart';

/// Kind of transport used to reach the daemon.
///
/// Only [direct] is implemented today; the enum exists so connection
/// settings can already express HTTP-over-SSH / HTTP-over-Tailscale
/// transports once they land (they will plug in as [DaemonTransport]
/// implementations).
enum TransportKind { direct, ssh, tailscale }

/// Persisted app settings: the list of configured daemon machines, which one
/// is active, plus UI preferences.
class AppSettings extends ChangeNotifier {
  AppSettings._();

  static const _kMachines = 'daemon.machines';
  static const _kActiveMachine = 'daemon.active_machine';
  static const _kRecentWorkspaces = 'ui.recent_workspaces';
  static const _kDefaultCapabilityMode = 'ui.default_capability_mode';
  static const _kAppLanguage = 'ui.app_language';

  /// Optional build-time seed values (development / CI convenience):
  /// `--dart-define=PHI_DAEMON_URL=... --dart-define=PHI_DAEMON_KEY=...`
  static const _seedUrl = String.fromEnvironment('PHI_DAEMON_URL');
  static const _seedKey = String.fromEnvironment('PHI_DAEMON_KEY');

  static const _uuid = Uuid();

  static Future<AppSettings> load() async {
    final settings = AppSettings._();
    final prefs = await SharedPreferences.getInstance();
    settings._prefs = prefs;
    await settings._loadMachines(prefs);
    settings._defaultCapabilityMode =
        prefs.getString(_kDefaultCapabilityMode) ?? 'workspace_edit';
    settings._appLanguage = prefs.getString(_kAppLanguage) ?? 'system';
    return settings;
  }

  Future<void> _loadMachines(SharedPreferences prefs) async {
    final stored = prefs.getString(_kMachines);
    if (stored != null) {
      _machines = _decodeMachines(stored);
      final activeId = prefs.getString(_kActiveMachine);
      if (activeId != null && _machines.any((m) => m.id == activeId)) {
        _activeMachineId = activeId;
      }
      return;
    }
    // Fresh install: seed the first machine from the build-time defines
    // when a key is provided.
    if (_seedKey.trim().isNotEmpty) {
      final machine = MachineConnection(
        id: _uuid.v4(),
        name: '',
        baseUrl: _seedUrl.trim().isNotEmpty
            ? _seedUrl.trim()
            : 'http://127.0.0.1:8787',
        authKey: _seedKey.trim(),
      );
      _machines = [machine];
      _activeMachineId = machine.id;
      await _persistMachines();
    }
  }

  static List<MachineConnection> _decodeMachines(String raw) {
    final Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } on FormatException {
      return <MachineConnection>[];
    }
    if (decoded is! List) return <MachineConnection>[];
    return [for (final entry in decoded) ?MachineConnection.tryFromJson(entry)];
  }

  late final SharedPreferences _prefs;

  List<MachineConnection> _machines = [];
  String? _activeMachineId;

  /// Per-machine recent workspaces, lazily loaded from
  /// `ui.recent_workspaces.<machineId>` keys.
  final Map<String, List<String>> _recentByMachine = {};
  String _defaultCapabilityMode = 'workspace_edit';
  String _appLanguage = 'system'; // system | en | zh

  /// All configured machines, in user order.
  List<MachineConnection> get machines => List.unmodifiable(_machines);

  /// The machine all transports/clients currently point at, if any.
  MachineConnection? get activeMachine {
    final id = _activeMachineId;
    if (id == null) return null;
    for (final machine in _machines) {
      if (machine.id == id) return machine;
    }
    return null;
  }

  /// Connection fields of the active machine (empty when none is selected).
  String get baseUrl => activeMachine?.baseUrl ?? '';
  String get authKey => activeMachine?.authKey ?? '';
  bool get allowUntrustedCerts => activeMachine?.allowUntrustedCerts ?? false;

  /// Recent workspaces of the active machine (empty when none is active).
  /// Paths are recorded per machine because a workspace that exists on one
  /// daemon's host usually does not exist on another.
  List<String> get recentWorkspaces {
    final machineId = _activeMachineId;
    if (machineId == null) return const [];
    return List.unmodifiable(_recentsFor(machineId));
  }

  static String _recentsKey(String machineId) =>
      '$_kRecentWorkspaces.$machineId';

  List<String> _recentsFor(String machineId) {
    return _recentByMachine.putIfAbsent(
      machineId,
      () => _prefs.getStringList(_recentsKey(machineId)) ?? <String>[],
    );
  }

  String get defaultCapabilityMode => _defaultCapabilityMode;

  /// Language override: `system`, `en` or `zh`.
  String get appLanguage => _appLanguage;

  bool get isConfigured => activeMachine?.isConfigured ?? false;

  DaemonTransport? _transport;

  /// The current transport. Rebuilt whenever the active machine changes or
  /// the active machine's connection settings are edited.
  DaemonTransport get transport {
    final existing = _transport;
    if (existing != null) return existing;
    final created = _buildTransport();
    _transport = created;
    return created;
  }

  DaemonTransport _buildTransport() {
    var url = baseUrl.trim();
    if (url.isEmpty) url = 'http://127.0.0.1:8787';
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
      url = 'http://$url';
    }
    return DirectDaemonTransport(
      baseUri: Uri.parse(url),
      authKey: authKey.trim(),
      allowUntrustedCerts: allowUntrustedCerts,
    );
  }

  void _replaceTransport() {
    _transport?.dispose();
    _transport = null;
  }

  Future<void> _persistMachines() async {
    await _prefs.setString(
      _kMachines,
      jsonEncode([for (final machine in _machines) machine.toJson()]),
    );
    final activeId = _activeMachineId;
    if (activeId != null) {
      await _prefs.setString(_kActiveMachine, activeId);
    } else {
      await _prefs.remove(_kActiveMachine);
    }
  }

  /// Adds a machine and returns it. When [makeActive] is true (or this is
  /// the first machine), it becomes the active machine.
  Future<MachineConnection> addMachine({
    String name = '',
    required String baseUrl,
    required String authKey,
    bool allowUntrustedCerts = false,
    bool makeActive = false,
  }) async {
    final machine = MachineConnection(
      id: _uuid.v4(),
      name: name.trim(),
      baseUrl: baseUrl.trim(),
      authKey: authKey.trim(),
      allowUntrustedCerts: allowUntrustedCerts,
    );
    _machines = [..._machines, machine];
    final activates = makeActive || _activeMachineId == null;
    if (activates) {
      _activeMachineId = machine.id;
    }
    await _persistMachines();
    if (activates) _replaceTransport();
    notifyListeners();
    return machine;
  }

  /// Replaces the machine with the same id. Editing the active machine
  /// rebuilds the transport; editing any other machine does not.
  Future<void> updateMachine(MachineConnection updated) async {
    final index = _machines.indexWhere((m) => m.id == updated.id);
    if (index < 0) return;
    _machines = [..._machines]..[index] = updated;
    await _persistMachines();
    if (updated.id == _activeMachineId) _replaceTransport();
    notifyListeners();
  }

  /// Removes a machine (and its recorded recent workspaces). Removing the
  /// active machine clears the selection (the app falls back to the
  /// not-configured state); it never silently switches to another machine.
  Future<void> removeMachine(String id) async {
    if (!_machines.any((m) => m.id == id)) return;
    _machines = _machines.where((m) => m.id != id).toList();
    final wasActive = _activeMachineId == id;
    if (wasActive) _activeMachineId = null;
    _recentByMachine.remove(id);
    await _prefs.remove(_recentsKey(id));
    await _persistMachines();
    if (wasActive) _replaceTransport();
    notifyListeners();
  }

  /// Selects which machine the app connects to.
  Future<void> setActiveMachine(String id) async {
    if (!_machines.any((m) => m.id == id)) return;
    if (_activeMachineId == id) return;
    _activeMachineId = id;
    await _persistMachines();
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
    final machineId = _activeMachineId;
    // Recents are per machine: a path recorded on one daemon may not exist
    // on another, so without an active machine there is nothing to record.
    if (machineId == null) return;
    final recents = _recentsFor(machineId);
    final updated = [path, ...recents.where((p) => p != path)];
    if (updated.length > 12) {
      updated.removeRange(12, updated.length);
    }
    _recentByMachine[machineId] = updated;
    await _prefs.setStringList(_recentsKey(machineId), updated);
    notifyListeners();
  }
}
