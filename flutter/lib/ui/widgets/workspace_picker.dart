import 'package:flutter/material.dart';

import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';
import '../../state/daemon_client.dart';

/// Result of the new-session dialog.
class NewSessionConfig {
  const NewSessionConfig({
    this.workspace,
    this.capabilityMode = 'workspace_edit',
    this.profileId = 'default',
  });

  final String? workspace;
  final String capabilityMode;
  final String profileId;
}

/// Dialog that collects workspace + provider profile + capability mode for a
/// new session.
Future<NewSessionConfig?> showNewSessionDialog(
  BuildContext context,
  AppState app,
) {
  return showDialog<NewSessionConfig>(
    context: context,
    builder: (context) => _NewSessionDialog(app: app),
  );
}

class _NewSessionDialog extends StatefulWidget {
  const _NewSessionDialog({required this.app});

  final AppState app;

  @override
  State<_NewSessionDialog> createState() => _NewSessionDialogState();
}

class _NewSessionDialogState extends State<_NewSessionDialog> {
  String? _workspace;
  String _capabilityMode = CapabilityMode.workspaceEdit;
  String _profileId = 'default';
  late final Future<List<PublicProviderConfig>> _providers = widget.app.client
      .listProviders();

  @override
  void initState() {
    super.initState();
    _capabilityMode = widget.app.settings.defaultCapabilityMode;
    final recents = widget.app.settings.recentWorkspaces;
    if (recents.isNotEmpty) _workspace = recents.first;
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final recents = widget.app.settings.recentWorkspaces;
    return AlertDialog(
      title: Text(s.newSession),
      content: SizedBox(
        width: 420,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      _workspace ?? s.noWorkspaceDefault,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodyMedium,
                    ),
                  ),
                  TextButton.icon(
                    onPressed: () async {
                      final picked = await showWorkspaceBrowser(
                        context,
                        widget.app.client,
                        initialPath: _workspace,
                      );
                      if (picked != null) {
                        setState(() => _workspace = picked);
                      }
                    },
                    icon: const Icon(Icons.folder_open, size: 18),
                    label: Text(s.browse),
                  ),
                ],
              ),
              if (recents.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    for (final path in recents.take(4))
                      ActionChip(
                        label: Text(
                          path.split('/').where((s) => s.isNotEmpty).last,
                          style: const TextStyle(fontSize: 12),
                        ),
                        tooltip: path,
                        onPressed: () => setState(() => _workspace = path),
                      ),
                  ],
                ),
              ],
              const SizedBox(height: 16),
              FutureBuilder<List<PublicProviderConfig>>(
                future: _providers,
                builder: (context, snapshot) {
                  final providers = snapshot.data;
                  // Provider profiles live on the daemon; the 'default'
                  // placeholder may not exist on the active machine. Fall
                  // back to the first real profile instead of submitting a
                  // stale id the daemon does not know.
                  if (providers != null &&
                      providers.isNotEmpty &&
                      !providers.any((p) => p.profileId == _profileId)) {
                    final fallback = providers.first.profileId;
                    WidgetsBinding.instance.addPostFrameCallback((_) {
                      if (mounted) setState(() => _profileId = fallback);
                    });
                  }
                  return DropdownButtonFormField<String>(
                    initialValue:
                        providers != null &&
                            providers.any((p) => p.profileId == _profileId)
                        ? _profileId
                        : null,
                    isExpanded: true,
                    decoration: InputDecoration(
                      labelText: s.providerProfile,
                      border: const OutlineInputBorder(),
                      isDense: true,
                    ),
                    items: [
                      for (final profile in providers ?? const [])
                        DropdownMenuItem(
                          value: profile.profileId,
                          child: Text(
                            '${profile.profileId} (${profile.model})',
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                    ],
                    onChanged: (value) {
                      if (value != null) setState(() => _profileId = value);
                    },
                  );
                },
              ),
              const SizedBox(height: 12),
              DropdownButtonFormField<String>(
                initialValue: _capabilityMode,
                isExpanded: true,
                decoration: InputDecoration(
                  labelText: s.capabilityMode,
                  border: const OutlineInputBorder(),
                  isDense: true,
                ),
                items: [
                  for (final mode in CapabilityMode.all)
                    DropdownMenuItem(
                      value: mode,
                      child: Text(capabilityModeLabel(s, mode)),
                    ),
                ],
                onChanged: (value) {
                  if (value != null) setState(() => _capabilityMode = value);
                },
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(s.cancel),
        ),
        FilledButton(
          onPressed: () {
            final workspace = _workspace;
            if (workspace != null) {
              widget.app.settings.addRecentWorkspace(workspace);
            }
            Navigator.of(context).pop(
              NewSessionConfig(
                workspace: workspace,
                capabilityMode: _capabilityMode,
                profileId: _profileId,
              ),
            );
          },
          child: Text(s.start),
        ),
      ],
    );
  }
}

/// Full-screen directory browser backed by `GET /v1/workspaces/browse`.
/// Returns the chosen absolute directory path.
Future<String?> showWorkspaceBrowser(
  BuildContext context,
  DaemonClient client, {
  String? initialPath,
}) {
  return showDialog<String>(
    context: context,
    builder: (context) =>
        _WorkspaceBrowser(client: client, initialPath: initialPath),
  );
}

class _WorkspaceBrowser extends StatefulWidget {
  const _WorkspaceBrowser({required this.client, this.initialPath});

  final DaemonClient client;
  final String? initialPath;

  @override
  State<_WorkspaceBrowser> createState() => _WorkspaceBrowserState();
}

class _WorkspaceBrowserState extends State<_WorkspaceBrowser> {
  WorkspaceBrowseResponse? _current;
  Object? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _browse(widget.initialPath);
  }

  Future<void> _browse(String? path) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final result = await widget.client.browseWorkspace(path);
      if (!mounted) return;
      setState(() {
        _current = result;
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _error = error;
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final current = _current;
    return AlertDialog(
      title: Text(s.chooseWorkspace),
      content: SizedBox(
        width: 480,
        height: 420,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                IconButton(
                  icon: const Icon(Icons.arrow_upward, size: 20),
                  tooltip: s.parentDirectory,
                  onPressed: current?.parent != null
                      ? () => _browse(current!.parent)
                      : null,
                ),
                Expanded(
                  child: Text(
                    current?.path ?? widget.initialPath ?? '…',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
              ],
            ),
            const Divider(height: 8),
            Expanded(
              child: _loading
                  ? const Center(child: CircularProgressIndicator())
                  : _error != null
                  ? Center(child: Text('$_error'))
                  : current == null || current.directories.isEmpty
                  ? Center(child: Text(s.noSubdirectories))
                  : ListView.builder(
                      itemCount: current.directories.length,
                      itemBuilder: (context, index) {
                        final dir = current.directories[index];
                        return ListTile(
                          dense: true,
                          leading: const Icon(Icons.folder_outlined, size: 20),
                          title: Text(dir.name),
                          onTap: () => _browse(dir.path),
                        );
                      },
                    ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(s.cancel),
        ),
        FilledButton(
          onPressed: current == null
              ? null
              : () => Navigator.of(context).pop(current.path),
          child: Text(s.selectThisDirectory),
        ),
      ],
    );
  }
}
