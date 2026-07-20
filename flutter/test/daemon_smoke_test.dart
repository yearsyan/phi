/// Live-daemon smoke test: exercises the REST client and the WebSocket
/// session protocol against a real phi daemon at 127.0.0.1:8787.
///
/// Disabled unless `PHI_RUN_DAEMON_SMOKE_TEST=1` and
/// `PHI_DAEMON_AUTH_KEY_FILE` are provided. This keeps the default test suite
/// deterministic and independent of machine-local daemon state.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/wire.dart';
import 'package:phi_client/core/transport/direct_transport.dart';
import 'package:phi_client/state/daemon_client.dart';

String? _liveSmokeSkipReason() {
  if (Platform.environment['PHI_RUN_DAEMON_SMOKE_TEST'] != '1') {
    return 'set PHI_RUN_DAEMON_SMOKE_TEST=1 to enable live daemon tests';
  }
  final keyFile = Platform.environment['PHI_DAEMON_AUTH_KEY_FILE'];
  if (keyFile == null || keyFile.trim().isEmpty) {
    return 'set PHI_DAEMON_AUTH_KEY_FILE to enable live daemon tests';
  }
  return null;
}

Future<DaemonClient> _createClient() async {
  final keyFile = Platform.environment['PHI_DAEMON_AUTH_KEY_FILE']!;
  final key = (await File(keyFile).readAsString()).trim();
  final transport = DirectDaemonTransport(
    baseUri: Uri.parse(
      Platform.environment['PHI_DAEMON_URL'] ?? 'http://127.0.0.1:8787',
    ),
    authKey: key,
  );
  final client = DaemonClient(transport);
  await client.listSessions();
  return client;
}

void main() {
  final skipReason = _liveSmokeSkipReason();
  DaemonClient? client;

  setUpAll(() async {
    if (skipReason == null) {
      client = await _createClient();
    }
  });

  test('REST: list sessions + browse workspace', () async {
    final c = client!;
    final sessions = await c.listSessions();
    expect(sessions.sessions, isA<List<SessionSummary>>());
    final browse = await c.browseWorkspace(Directory.current.absolute.path);
    expect(browse.directories, isNotEmpty);
  }, skip: skipReason);

  test(
    'WS: new session runs a prompt end to end',
    () async {
      final c = client!;

      final token = await c.mintSocketToken();
      final socket = await c.transport.connect(
        '/v1/ws/new',
        protocols: ['phi.v1', 'phi.auth.$token'],
      );

      final completer = Completer<void>();
      String? sessionId;
      final events = <String>[];
      var sawReady = false;
      var sawAssistantText = false;
      var sent = false;

      final sub = socket.messages.listen((raw) {
        final frame = ServerFrame.parse(raw);
        events.add(frame.type);
        switch (frame.type) {
          case 'ready':
            sawReady = true;
            if (!sent) {
              sent = true;
              socket.send(
                ClientCommand.prompt(
                  'test-1',
                  const Content.text('Reply with exactly one word: ok'),
                ).encode(),
              );
            }
          case 'session_created':
            sessionId = frame.sessionId;
          case 'event':
            final envelope = frame.envelope;
            final event = envelope?.event;
            if (event == null) break;
            if (event.type == 'message_update' && event.delta?.kind == 'text') {
              sawAssistantText = true;
            }
            if (event.type == 'run_completed' || event.type == 'run_failed') {
              if (!completer.isCompleted) completer.complete();
            }
          case 'fatal_error':
            if (!completer.isCompleted) {
              completer.completeError(StateError('fatal: ${frame.message}'));
            }
        }
      });

      await completer.future.timeout(const Duration(seconds: 180));
      await sub.cancel();
      await socket.close();

      expect(sawReady, isTrue);
      expect(sessionId, isNotNull);
      expect(sawAssistantText, isTrue);
      expect(events, isNot(contains('fatal_error')));

      // Cleanup: delete the session created by this test.
      if (sessionId != null) {
        await c.deleteSession(sessionId!);
      }
    },
    timeout: const Timeout(Duration(minutes: 4)),
    skip: skipReason,
  );
}
