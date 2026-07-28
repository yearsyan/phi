import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/platform/secure_storage.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('macOS selects the prompt-free native Keychain adapter', () {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);

    expect(defaultSecureStorage(), isA<MacOsPromptFreeKeychainStore>());
  });

  test('native Keychain adapter forwards read, write, and delete', () async {
    const channel = MethodChannel(
      'dev.phi.phi_client/prompt_free_keychain.test',
    );
    final calls = <MethodCall>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      return call.method == 'read' ? 'fixture-daemon-key' : null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    const store = MacOsPromptFreeKeychainStore(channel: channel);

    expect(await store.read('machine-key'), 'fixture-daemon-key');
    await store.write('machine-key', 'replacement-fixture-key');
    await store.delete('machine-key');

    expect(
      [for (final call in calls) call.method],
      ['read', 'write', 'delete'],
    );
    expect(calls[0].arguments, <String, Object?>{'key': 'machine-key'});
    expect(calls[1].arguments, <String, Object?>{
      'key': 'machine-key',
      'value': 'replacement-fixture-key',
    });
    expect(calls[2].arguments, <String, Object?>{'key': 'machine-key'});
  });
}
