import 'package:flutter/foundation.dart';

import '../core/settings/app_settings.dart';
import '../core/transport/daemon_transport.dart';
import 'daemon_client.dart';
import 'sessions_store.dart';

/// Root application state: owns settings, the current daemon client and the
/// sessions store. When connection settings change, the transport (and hence
/// client/store) is rebuilt and listeners are notified.
class AppState extends ChangeNotifier {
  AppState(this.settings, {DaemonTransport? transportOverride})
    : _transportOverride = transportOverride {
    client = DaemonClient(_transport);
    sessionsStore = SessionsStore(client);
    _lastTransport = _transport;
    settings.addListener(_onSettingsChanged);
  }

  final AppSettings settings;
  late DaemonClient client;
  late SessionsStore sessionsStore;

  /// Test hook: when set, this transport is always used instead of the one
  /// built from [AppSettings] (keeps widget tests off the network).
  final DaemonTransport? _transportOverride;

  DaemonTransport get _transport => _transportOverride ?? settings.transport;

  Object? _lastTransport;

  void _onSettingsChanged() {
    final transport = _transport;
    if (!identical(transport, _lastTransport)) {
      _lastTransport = transport;
      client = DaemonClient(transport);
      sessionsStore.dispose();
      sessionsStore = SessionsStore(client);
      notifyListeners();
    }
  }

  @override
  void dispose() {
    settings.removeListener(_onSettingsChanged);
    sessionsStore.dispose();
    super.dispose();
  }
}
