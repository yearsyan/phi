import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/daemon_client.dart';
import 'package:phi_client/state/sessions_store.dart';

class _SessionsTransport implements DaemonTransport {
  _SessionsTransport(this.sessionId, {this.gate});

  final String sessionId;
  final Completer<void>? gate;
  int listRequests = 0;

  @override
  String get displayName => sessionId;

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    if (method != 'GET' || path != '/v1/sessions') {
      throw UnsupportedError('unexpected request: $method $path');
    }
    listRequests++;
    await gate?.future;
    return DaemonHttpResponse(
      200,
      '{"sessions":[{"session_id":"$sessionId","config":{}}],'
      '"workspaces":[]}',
      const {},
    );
  }

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async => throw UnsupportedError('sockets are not used by this test');

  @override
  void dispose() {}
}

void main() {
  test(
    'active store requests sessions once after the daemon changes',
    () async {
      final oldTransport = _SessionsTransport('old');
      final newTransport = _SessionsTransport('new');
      final store = SessionsStore(DaemonClient(oldTransport));
      addTearDown(store.dispose);

      await store.activate();
      await store.activate();
      await store.replaceClient(DaemonClient(newTransport));

      expect(oldTransport.listRequests, 1);
      expect(newTransport.listRequests, 1);
      expect(store.sessions.single.sessionId, 'new');
    },
  );

  test(
    'a late response from the old daemon cannot replace the new list',
    () async {
      final oldGate = Completer<void>();
      final oldTransport = _SessionsTransport('old', gate: oldGate);
      final newTransport = _SessionsTransport('new');
      final store = SessionsStore(DaemonClient(oldTransport));
      addTearDown(store.dispose);

      final oldRefresh = store.activate();
      await store.replaceClient(DaemonClient(newTransport));
      oldGate.complete();
      await oldRefresh;

      expect(store.sessions.single.sessionId, 'new');
    },
  );
}
