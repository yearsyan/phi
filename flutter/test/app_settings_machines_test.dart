import 'package:flutter_test/flutter_test.dart';
import 'package:phi_client/core/settings/app_settings.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  // Fixture values only — never real credentials.
  const keyA = 'fixture-key-a';
  const keyB = 'fixture-key-b';

  test('fresh install starts with no machines and is not configured', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();

    expect(settings.machines, isEmpty);
    expect(settings.activeMachine, isNull);
    expect(settings.isConfigured, isFalse);
  });

  test(
    'addMachine persists, notifies, and activates the first machine',
    () async {
      SharedPreferences.setMockInitialValues({});
      final settings = await AppSettings.load();
      var notified = 0;
      settings.addListener(() => notified++);

      final machine = await settings.addMachine(
        name: 'Workstation',
        baseUrl: '192.0.2.10:8787',
        authKey: keyA,
      );

      expect(notified, 1);
      expect(settings.machines, [machine]);
      expect(settings.activeMachine?.id, machine.id);
      expect(settings.isConfigured, isTrue);
      expect(machine.baseUrl, '192.0.2.10:8787');

      // Survives a reload.
      final reloaded = await AppSettings.load();
      expect(reloaded.machines.single.id, machine.id);
      expect(reloaded.activeMachine?.id, machine.id);
    },
  );

  test('adding a second machine does not steal the active selection', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final first = await settings.addMachine(
      baseUrl: 'http://a:1',
      authKey: keyA,
    );
    final second = await settings.addMachine(
      baseUrl: 'http://b:2',
      authKey: keyB,
    );

    expect(settings.machines, hasLength(2));
    expect(settings.activeMachine?.id, first.id);
    expect(settings.activeMachine?.id, isNot(second.id));
  });

  test('setActiveMachine switches transport identity', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    await settings.addMachine(baseUrl: 'http://a:1', authKey: keyA);
    final second = await settings.addMachine(
      baseUrl: 'http://b:2',
      authKey: keyB,
    );
    final before = settings.transport;

    await settings.setActiveMachine(second.id);

    expect(settings.activeMachine?.id, second.id);
    expect(settings.transport, isNot(same(before)));

    // Re-selecting the same machine is a no-op.
    final current = settings.transport;
    await settings.setActiveMachine(second.id);
    expect(settings.transport, same(current));
  });

  test('updateMachine on the active machine rebuilds the transport', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final active = await settings.addMachine(
      baseUrl: 'http://a:1',
      authKey: keyA,
    );
    final other = await settings.addMachine(
      baseUrl: 'http://b:2',
      authKey: keyB,
    );

    // Editing a non-active machine keeps the transport instance.
    final transport = settings.transport;
    await settings.updateMachine(other.copyWith(name: 'B'));
    expect(settings.transport, same(transport));

    // Editing the active machine rebuilds it.
    await settings.updateMachine(active.copyWith(baseUrl: 'http://a:9999'));
    expect(settings.transport, isNot(same(transport)));
    expect(settings.activeMachine?.baseUrl, 'http://a:9999');
  });

  test('removing the active machine clears the selection', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final active = await settings.addMachine(
      baseUrl: 'http://a:1',
      authKey: keyA,
    );
    final other = await settings.addMachine(
      baseUrl: 'http://b:2',
      authKey: keyB,
    );

    await settings.removeMachine(active.id);

    expect(settings.machines, hasLength(1));
    expect(settings.activeMachine, isNull);
    expect(settings.isConfigured, isFalse);
    // Never silently switches to another machine.
    expect(settings.activeMachine?.id, isNot(other.id));

    final reloaded = await AppSettings.load();
    expect(reloaded.activeMachine, isNull);
  });

  test('removing a non-active machine keeps the active selection', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final active = await settings.addMachine(
      baseUrl: 'http://a:1',
      authKey: keyA,
    );
    final other = await settings.addMachine(
      baseUrl: 'http://b:2',
      authKey: keyB,
    );

    await settings.removeMachine(other.id);

    expect(settings.machines.single.id, active.id);
    expect(settings.activeMachine?.id, active.id);
  });

  test('corrupt stored machine data decodes to an empty list', () async {
    SharedPreferences.setMockInitialValues({
      'daemon.machines': '{not json',
      'daemon.active_machine': 'gone',
    });
    final settings = await AppSettings.load();

    expect(settings.machines, isEmpty);
    expect(settings.activeMachine, isNull);
  });

  test('stored active id pointing at a missing machine is ignored', () async {
    SharedPreferences.setMockInitialValues({
      'daemon.machines':
          '[{"id":"m-1","name":"","base_url":"http://a:1","auth_key":"$keyA"}]',
      'daemon.active_machine': 'no-such-machine',
    });
    final settings = await AppSettings.load();

    expect(settings.machines, hasLength(1));
    expect(settings.activeMachine, isNull);
  });

  test('recent workspaces are scoped per machine', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final a = await settings.addMachine(baseUrl: 'http://a:1', authKey: keyA);
    final b = await settings.addMachine(baseUrl: 'http://b:2', authKey: keyB);

    await settings.addRecentWorkspace('/home/a/project');
    expect(settings.recentWorkspaces, ['/home/a/project']);

    // Switching to machine B must not show A's paths.
    await settings.setActiveMachine(b.id);
    expect(settings.recentWorkspaces, isEmpty);

    await settings.addRecentWorkspace('/Users/b/project');
    expect(settings.recentWorkspaces, ['/Users/b/project']);

    // A's list is untouched, and survives a reload.
    await settings.setActiveMachine(a.id);
    expect(settings.recentWorkspaces, ['/home/a/project']);
    final reloaded = await AppSettings.load();
    expect(reloaded.recentWorkspaces, ['/home/a/project']);

    await reloaded.setActiveMachine(b.id);
    expect(reloaded.recentWorkspaces, ['/Users/b/project']);
  });

  test('addRecentWorkspace without an active machine is a no-op', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();

    await settings.addRecentWorkspace('/nowhere');
    expect(settings.recentWorkspaces, isEmpty);
  });

  test('removing a machine also removes its recents', () async {
    SharedPreferences.setMockInitialValues({});
    final settings = await AppSettings.load();
    final machine = await settings.addMachine(
      baseUrl: 'http://a:1',
      authKey: keyA,
    );
    await settings.addRecentWorkspace('/home/a/project');

    await settings.removeMachine(machine.id);

    final prefs = await SharedPreferences.getInstance();
    expect(prefs.getStringList('ui.recent_workspaces.${machine.id}'), isNull);
  });
}
