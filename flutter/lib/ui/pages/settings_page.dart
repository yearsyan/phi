import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import 'machines_page.dart';

/// App settings: entry point to machine management plus default preferences
/// (language, default capability mode for new sessions).
class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key, this.embedded = false});

  final bool embedded;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final app = AppScope.of(context);
    final settings = app.settings;
    final s = S.of(context);
    return Scaffold(
      appBar: AppBar(
        automaticallyImplyLeading: !embedded,
        title: Text(s.settings),
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Align(
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 560),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(s.daemonConnection, style: theme.textTheme.titleSmall),
                  const SizedBox(height: 4),
                  Text(
                    s.daemonConnectionDescription,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                  const SizedBox(height: 8),
                  ListenableBuilder(
                    listenable: settings,
                    builder: (context, _) {
                      final active = settings.activeMachine;
                      return Card(
                        clipBehavior: Clip.antiAlias,
                        child: ListTile(
                          leading: const Icon(Icons.dns_outlined),
                          title: Text(
                            s.machinesCount(settings.machines.length),
                          ),
                          subtitle: Text(
                            active != null
                                ? '${s.activeMachine}: ${active.displayName}'
                                : s.noMachinesYet,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                          ),
                          trailing: const Icon(Icons.chevron_right),
                          onTap: () => Navigator.of(context).push(
                            MaterialPageRoute<void>(
                              builder: (_) => const MachinesPage(),
                            ),
                          ),
                        ),
                      );
                    },
                  ),
                  const SizedBox(height: 16),
                  const Divider(),
                  const SizedBox(height: 12),
                  Text(s.defaults, style: theme.textTheme.titleSmall),
                  const SizedBox(height: 8),
                  DropdownButtonFormField<String>(
                    initialValue: settings.appLanguage,
                    decoration: InputDecoration(
                      labelText: s.language,
                      border: const OutlineInputBorder(),
                    ),
                    items: [
                      DropdownMenuItem(
                        value: 'system',
                        child: Text(s.languageSystem),
                      ),
                      const DropdownMenuItem(
                        value: 'en',
                        child: Text('English'),
                      ),
                      const DropdownMenuItem(value: 'zh', child: Text('中文')),
                    ],
                    onChanged: (value) {
                      if (value != null) {
                        settings.setAppLanguage(value);
                      }
                    },
                  ),
                  const SizedBox(height: 12),
                  DropdownButtonFormField<String>(
                    initialValue: settings.defaultCapabilityMode,
                    decoration: InputDecoration(
                      labelText: s.defaultCapabilityMode,
                      border: const OutlineInputBorder(),
                    ),
                    items: [
                      for (final mode in CapabilityMode.all)
                        DropdownMenuItem(
                          value: mode,
                          child: Text(capabilityModeLabel(s, mode)),
                        ),
                    ],
                    onChanged: (value) {
                      if (value != null) {
                        settings.setDefaultCapabilityMode(value);
                      }
                    },
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
