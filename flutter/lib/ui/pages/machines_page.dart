import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/machine_connection.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';
import '../widgets/machine_editor.dart';

/// Machine management: list all configured daemon machines, add/edit/delete
/// them and pick which one is active.
class MachinesPage extends StatelessWidget {
  const MachinesPage({super.key});

  AppState _app(BuildContext context) => AppScope.of(context);

  Future<void> _confirmDelete(
    BuildContext context,
    MachineConnection machine,
    bool isActive,
  ) async {
    final s = S.of(context);
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(s.deleteMachineTitle),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(s.deleteMachineBody(machine.displayName)),
            if (isActive) ...[
              const SizedBox(height: 8),
              Text(
                s.deleteActiveMachineHint,
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                  color: Theme.of(context).colorScheme.error,
                ),
              ),
            ],
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(s.cancel),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: Theme.of(context).colorScheme.error,
            ),
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(s.delete),
          ),
        ],
      ),
    );
    if (confirmed == true && context.mounted) {
      await _app(context).settings.removeMachine(machine.id);
      if (context.mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(s.machineDeleted)));
      }
    }
  }

  void _showActions(
    BuildContext pageContext,
    MachineConnection machine,
    bool isActive,
  ) {
    final s = S.of(pageContext);
    final settings = _app(pageContext).settings;
    showModalBottomSheet<void>(
      context: pageContext,
      showDragHandle: true,
      builder: (sheetContext) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (!isActive)
              ListTile(
                leading: const Icon(Icons.check_circle_outline),
                title: Text(s.setActive),
                onTap: () {
                  Navigator.of(sheetContext).pop();
                  settings.setActiveMachine(machine.id);
                },
              ),
            ListTile(
              leading: const Icon(Icons.edit_outlined),
              title: Text(s.editMachine),
              onTap: () {
                Navigator.of(sheetContext).pop();
                showMachineEditor(pageContext, existing: machine);
              },
            ),
            ListTile(
              leading: Icon(
                Icons.delete_outline,
                color: Theme.of(sheetContext).colorScheme.error,
              ),
              title: Text(
                s.delete,
                style: TextStyle(
                  color: Theme.of(sheetContext).colorScheme.error,
                ),
              ),
              onTap: () {
                Navigator.of(sheetContext).pop();
                _confirmDelete(pageContext, machine, isActive);
              },
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final settings = _app(context).settings;
    return Scaffold(
      appBar: AppBar(title: Text(s.machines)),
      body: ListenableBuilder(
        listenable: settings,
        builder: (context, _) {
          final machines = settings.machines;
          final activeId = settings.activeMachine?.id;
          if (machines.isEmpty) {
            return Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      Icons.dns_outlined,
                      size: 40,
                      color: theme.colorScheme.outline,
                    ),
                    const SizedBox(height: 12),
                    Text(s.noMachinesYet, style: theme.textTheme.titleSmall),
                    const SizedBox(height: 6),
                    Text(
                      s.noMachinesHint,
                      textAlign: TextAlign.center,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                    const SizedBox(height: 12),
                    FilledButton.icon(
                      onPressed: () => showMachineEditor(context),
                      icon: const Icon(Icons.add, size: 18),
                      label: Text(s.addMachine),
                    ),
                  ],
                ),
              ),
            );
          }
          return ListView(
            padding: const EdgeInsets.symmetric(vertical: 8),
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
                child: Text(
                  s.daemonConnectionDescription,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
              for (final machine in machines)
                _MachineTile(
                  machine: machine,
                  active: machine.id == activeId,
                  onTap: () => showMachineEditor(context, existing: machine),
                  onLongPress: () =>
                      _showActions(context, machine, machine.id == activeId),
                ),
              const SizedBox(height: 8),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 16),
                child: OutlinedButton.icon(
                  onPressed: () => showMachineEditor(context),
                  icon: const Icon(Icons.add, size: 18),
                  label: Text(s.addMachine),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _MachineTile extends StatelessWidget {
  const _MachineTile({
    required this.machine,
    required this.active,
    required this.onTap,
    required this.onLongPress,
  });

  final MachineConnection machine;
  final bool active;
  final VoidCallback onTap;
  final VoidCallback onLongPress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    return ListTile(
      onTap: onTap,
      onLongPress: onLongPress,
      leading: Icon(
        active ? Icons.cloud_done_outlined : Icons.cloud_outlined,
        color: active ? theme.colorScheme.primary : theme.colorScheme.outline,
      ),
      title: Row(
        children: [
          Flexible(
            child: Text(
              machine.displayName,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          if (machine.allowUntrustedCerts) ...[
            const SizedBox(width: 6),
            _Badge(label: s.selfSignedBadge, color: theme.colorScheme.outline),
          ],
        ],
      ),
      subtitle: Text(
        machine.baseUrl,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      trailing: active
          ? _Badge(label: s.activeMachine, color: theme.colorScheme.primary)
          : null,
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.label, required this.color});

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        border: Border.all(color: color.withAlpha(160)),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(
        label,
        style: Theme.of(context).textTheme.labelSmall?.copyWith(color: color),
      ),
    );
  }
}
