import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/machine_connection.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';
import '../pages/machines_page.dart';

/// App-bar title button showing the active machine and offering quick
/// switching between all configured machines.
class MachineSwitcher extends StatelessWidget {
  const MachineSwitcher({super.key});

  AppState _app(BuildContext context) => AppScope.of(context);

  Future<void> _openSwitcher(BuildContext context) async {
    final s = S.of(context);
    final settings = _app(context).settings;
    await showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (sheetContext) {
        return SafeArea(
          child: ListenableBuilder(
            listenable: settings,
            builder: (context, _) {
              final activeId = settings.activeMachine?.id;
              return Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final machine in settings.machines)
                    ListTile(
                      leading: Icon(
                        machine.id == activeId
                            ? Icons.check_circle
                            : Icons.cloud_outlined,
                        color: machine.id == activeId
                            ? Theme.of(context).colorScheme.primary
                            : Theme.of(context).colorScheme.outline,
                      ),
                      title: Text(
                        machine.displayName,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: Text(
                        machine.baseUrl,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      onTap: () {
                        Navigator.of(sheetContext).pop();
                        settings.setActiveMachine(machine.id);
                      },
                    ),
                  const Divider(height: 1),
                  ListTile(
                    leading: const Icon(Icons.settings_outlined),
                    title: Text(s.manageMachines),
                    onTap: () {
                      Navigator.of(sheetContext).pop();
                      Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          builder: (_) => const MachinesPage(),
                        ),
                      );
                    },
                  ),
                ],
              );
            },
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final settings = _app(context).settings;
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: settings,
      builder: (context, _) {
        final MachineConnection? active = settings.activeMachine;
        return InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: () => _openSwitcher(context),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(Icons.terminal_rounded, size: 22),
                const SizedBox(width: 8),
                Flexible(
                  child: Text(
                    active?.displayName ?? S.of(context).appTitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleLarge?.copyWith(fontSize: 18),
                  ),
                ),
                const SizedBox(width: 2),
                Icon(
                  Icons.unfold_more_rounded,
                  size: 18,
                  color: theme.colorScheme.outline,
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}
