import 'package:flutter/foundation.dart';

import '../core/settings/app_settings.dart';
import 'daemon_client.dart';
import 'sessions_store.dart';

/// Root application state: owns settings, the current daemon client and the
/// sessions store. When connection settings change, the transport (and hence
/// client/store) is rebuilt and listeners are notified.
class AppState extends ChangeNotifier {
  AppState(this.settings) {
    client = DaemonClient(settings.transport);
    sessionsStore = SessionsStore(client);
    _lastTransport = settings.transport;
    settings.addListener(_onSettingsChanged);
  }

  final AppSettings settings;
  late DaemonClient client;
  late SessionsStore sessionsStore;

  Object? _lastTransport;

  void _onSettingsChanged() {
    final transport = settings.transport;
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
