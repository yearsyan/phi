import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/connection_payload.dart';

void main() {
  // Fixture key: matches the daemon's 32..4096 byte rule but is not a real
  // credential. Never put a live daemon key in tests.
  const fixtureKey = 'test-fixture-key-0123456789abcdef';

  String payload({
    Object? type = 'phi-daemon',
    Object? version = 1,
    Object? baseUrl = 'http://192.0.2.10:8787',
    Object? authKey = fixtureKey,
  }) {
    final entries = <String>[
      if (type != null) '"type":${type is String ? '"$type"' : type}',
      if (version != null) '"version":$version',
      if (baseUrl != null) '"base_url":"$baseUrl"',
      if (authKey != null) '"auth_key":"$authKey"',
    ];
    return '{${entries.join(',')}}';
  }

  test('parses a valid daemon payload', () {
    final result = ConnectionPayload.tryParse(payload());
    expect(result, isNotNull);
    expect(result!.baseUrl, 'http://192.0.2.10:8787');
    expect(result.authKey, fixtureKey);
  });

  test('parses an https payload', () {
    final result = ConnectionPayload.tryParse(
      payload(baseUrl: 'https://[2001:db8::1]:9443'),
    );
    expect(result, isNotNull);
    expect(result!.baseUrl, 'https://[2001:db8::1]:9443');
  });

  test('trims surrounding whitespace in fields', () {
    final result = ConnectionPayload.tryParse(
      payload(baseUrl: ' http://192.0.2.10:8787 ', authKey: ' $fixtureKey '),
    );
    expect(result, isNotNull);
    expect(result!.baseUrl, 'http://192.0.2.10:8787');
    expect(result.authKey, fixtureKey);
  });

  test('rejects non-JSON input', () {
    expect(ConnectionPayload.tryParse('not json at all'), isNull);
    expect(ConnectionPayload.tryParse(''), isNull);
    expect(ConnectionPayload.tryParse('[1,2,3]'), isNull);
    expect(ConnectionPayload.tryParse('"just a string"'), isNull);
  });

  test('rejects a wrong payload type', () {
    expect(ConnectionPayload.tryParse(payload(type: 'other-app')), isNull);
  });

  test('rejects an unsupported version', () {
    expect(ConnectionPayload.tryParse(payload(version: 2)), isNull);
    expect(ConnectionPayload.tryParse(payload(version: '"1"')), isNull);
  });

  test('rejects missing fields', () {
    expect(ConnectionPayload.tryParse(payload(type: null)), isNull);
    expect(ConnectionPayload.tryParse(payload(version: null)), isNull);
    expect(ConnectionPayload.tryParse(payload(baseUrl: null)), isNull);
    expect(ConnectionPayload.tryParse(payload(authKey: null)), isNull);
  });

  test('rejects non-http(s) or relative base URLs', () {
    expect(
      ConnectionPayload.tryParse(payload(baseUrl: 'ftp://192.0.2.10:8787')),
      isNull,
    );
    expect(
      ConnectionPayload.tryParse(payload(baseUrl: '192.0.2.10:8787')),
      isNull,
    );
    expect(ConnectionPayload.tryParse(payload(baseUrl: 'http://')), isNull);
  });

  test('rejects an empty auth key', () {
    expect(ConnectionPayload.tryParse(payload(authKey: '')), isNull);
    expect(ConnectionPayload.tryParse(payload(authKey: '   ')), isNull);
  });
}
