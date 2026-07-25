import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/transport/daemon_transport.dart';
import 'package:phi_client/state/daemon_client.dart';

void main() {
  test('bot account and recipient target use separate API payloads', () async {
    final transport = _RecordingTransport();
    final client = DaemonClient(transport);

    final account = await client.putTelegramBotAccount(
      botAccountId: 'primary',
      botToken: '123456789:test_bot_token_with_enough_chars',
    );
    expect(transport.requests.first, {
      'method': 'PUT',
      'path': '/v1/bot-accounts/primary',
      'body': {
        'type': 'telegram',
        'bot_token': '123456789:test_bot_token_with_enough_chars',
      },
    });
    expect(account.botAccountId, 'primary');
    expect(account.botTokenConfigured, isTrue);

    final target = await client.putTelegramOutputChannel(
      outputChannelId: 'alice-alerts',
      botAccountId: 'primary',
      chatId: '5050551393',
    );
    expect(transport.requests.last, {
      'method': 'PUT',
      'path': '/v1/output-channels/alice-alerts',
      'body': {
        'type': 'telegram',
        'bot_account_id': 'primary',
        'chat_id': '5050551393',
      },
    });
    expect(target.outputChannelId, 'alice-alerts');
    expect(target.botAccountId, 'primary');
  });
}

class _RecordingTransport implements DaemonTransport {
  final List<Map<String, Object?>> requests = [];

  @override
  String get displayName => 'recording';

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    requests.add({'method': method, 'path': path, 'body': body});
    if (path.startsWith('/v1/bot-accounts/')) {
      return DaemonHttpResponse(
        200,
        jsonEncode({
          'configured': true,
          'bot_account': {
            'type': 'telegram',
            'bot_account_id': 'primary',
            'revision': 1,
            'bot_token_configured': true,
          },
        }),
        const {},
      );
    }
    return DaemonHttpResponse(
      200,
      jsonEncode({
        'configured': true,
        'output_channel': {
          'type': 'telegram',
          'output_channel_id': 'alice-alerts',
          'revision': 1,
          'bot_account_id': 'primary',
          'bot_token_configured': true,
          'chat_id': '5050551393',
        },
      }),
      const {},
    );
  }

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async => throw UnsupportedError('not used');

  @override
  void dispose() {}
}
