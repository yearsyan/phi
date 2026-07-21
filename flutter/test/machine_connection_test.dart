import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/models/machine_connection.dart';

void main() {
  // Fixture value only — never a real credential.
  const fixtureKey = 'test-fixture-key-not-a-real-secret';

  MachineConnection sample() => const MachineConnection(
    id: 'machine-1',
    name: 'Workstation',
    baseUrl: 'http://192.0.2.10:8787',
    authKey: fixtureKey,
    allowUntrustedCerts: true,
  );

  test('JSON round trip preserves all fields', () {
    final machine = sample();
    final restored = MachineConnection.tryFromJson(
      jsonDecode(jsonEncode(machine.toJson())),
    );
    expect(restored, isNotNull);
    expect(restored!.id, machine.id);
    expect(restored.name, machine.name);
    expect(restored.baseUrl, machine.baseUrl);
    expect(restored.authKey, machine.authKey);
    expect(restored.allowUntrustedCerts, machine.allowUntrustedCerts);
  });

  test('tolerant parsing fills missing fields with defaults', () {
    final machine = MachineConnection.tryFromJson({'id': 'm-2'});
    expect(machine, isNotNull);
    expect(machine!.name, '');
    expect(machine.baseUrl, '');
    expect(machine.authKey, '');
    expect(machine.allowUntrustedCerts, isFalse);
  });

  test('entries without a usable id are rejected', () {
    expect(MachineConnection.tryFromJson({'name': 'x'}), isNull);
    expect(MachineConnection.tryFromJson({'id': '  '}), isNull);
    expect(MachineConnection.tryFromJson('not a map'), isNull);
    expect(MachineConnection.tryFromJson(null), isNull);
  });

  test('unknown fields are ignored', () {
    final machine = MachineConnection.tryFromJson({
      'id': 'm-3',
      'name': 'n',
      'future_field': 42,
    });
    expect(machine, isNotNull);
    expect(machine!.name, 'n');
  });

  test('displayName prefers the user name', () {
    expect(sample().displayName, 'Workstation');
  });

  test('displayName falls back to host:port', () {
    const machine = MachineConnection(
      id: 'm-4',
      name: '',
      baseUrl: 'https://daemons.example.com:9443',
      authKey: fixtureKey,
    );
    expect(machine.displayName, 'daemons.example.com:9443');
  });

  test('displayName handles host without port and unparseable URLs', () {
    const hostOnly = MachineConnection(
      id: 'm-5',
      name: ' ',
      baseUrl: 'http://192.0.2.20:8787',
      authKey: fixtureKey,
    );
    expect(hostOnly.displayName, '192.0.2.20:8787');

    const empty = MachineConnection(
      id: 'm-6',
      name: '',
      baseUrl: '',
      authKey: '',
    );
    expect(empty.displayName, 'm-6');
  });

  test('isConfigured requires url and key', () {
    expect(sample().isConfigured, isTrue);
    expect(
      const MachineConnection(
        id: 'm-7',
        name: '',
        baseUrl: 'http://192.0.2.10:8787',
        authKey: '',
      ).isConfigured,
      isFalse,
    );
  });

  test('toString never contains the auth key', () {
    expect(sample().toString(), isNot(contains(fixtureKey)));
  });

  test('copyWith replaces only the given fields', () {
    final renamed = sample().copyWith(name: 'Renamed');
    expect(renamed.id, 'machine-1');
    expect(renamed.name, 'Renamed');
    expect(renamed.baseUrl, sample().baseUrl);
    expect(renamed.authKey, sample().authKey);
    expect(renamed.allowUntrustedCerts, isTrue);
  });
}
