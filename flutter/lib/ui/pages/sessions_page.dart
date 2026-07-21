import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../app.dart';
import '../../core/models/connection_payload.dart';
import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../platform/qr_scan_support.dart';
import '../../state/app_state.dart';
import 'scan_connection_page.dart';

/// Sessions sidebar: workspace-grouped session list with pin/delete actions.
class SessionsPage extends StatefulWidget {
  const SessionsPage({
    super.key,
    required this.embedded,
    required this.selectedSessionId,
    required this.onOpenSession,
    required this.onNewSession,
    required this.onOpenTasks,
    required this.onOpenSettings,
  });

  final bool embedded;
  final String? selectedSessionId;
  final ValueChanged<String> onOpenSession;
  final VoidCallback onNewSession;
  final VoidCallback onOpenTasks;
  final VoidCallback onOpenSettings;

  @override
  State<SessionsPage> createState() => _SessionsPageState();
}

class _SessionsPageState extends State<SessionsPage> {
  String _filter = '';

  AppState get _app => AppScope.of(context);

  @override
  Widget build(BuildContext context) {
    final store = _app.sessionsStore;
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        title: const Row(
          children: [
            Icon(Icons.terminal_rounded, size: 22),
            SizedBox(width: 8),
            Text('Phi'),
          ],
        ),
        actions: [
          if (!widget.embedded)
            IconButton(
              tooltip: S.of(context).newSession,
              icon: const Icon(Icons.add),
              onPressed: widget.onNewSession,
            ),
          IconButton(
            tooltip: S.of(context).scheduledTasks,
            icon: const Icon(Icons.schedule_rounded),
            onPressed: widget.onOpenTasks,
          ),
          IconButton(
            tooltip: S.of(context).settings,
            icon: const Icon(Icons.settings_outlined),
            onPressed: widget.onOpenSettings,
          ),
        ],
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
            child: TextField(
              decoration: InputDecoration(
                isDense: true,
                hintText: S.of(context).filterSessions,
                prefixIcon: const Icon(Icons.search, size: 18),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(10),
                ),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 10,
                ),
              ),
              onChanged: (value) => setState(() => _filter = value),
            ),
          ),
          if (widget.embedded)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
              child: SizedBox(
                width: double.infinity,
                child: OutlinedButton.icon(
                  onPressed: widget.onNewSession,
                  icon: const Icon(Icons.add, size: 18),
                  label: Text(S.of(context).newSession),
                ),
              ),
            ),
          Expanded(
            child: ListenableBuilder(
              listenable: store,
              builder: (context, _) => _buildList(context, store, theme),
            ),
          ),
        ],
      ),
    );
  }

  Future<void> _scanToConnect() async {
    final payload = await Navigator.of(context).push<ConnectionPayload>(
      MaterialPageRoute(builder: (_) => const ScanConnectionPage()),
    );
    if (payload == null || !mounted) return;
    await _app.settings.updateConnection(
      baseUrl: payload.baseUrl,
      authKey: payload.authKey,
    );
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(S.of(context).settingsSaved)));
    }
  }

  Widget _buildList(BuildContext context, dynamic store, ThemeData theme) {
    final s = S.of(context);
    if (!_app.settings.isConfigured) {
      return _CenteredHint(
        icon: Icons.key_off_outlined,
        title: s.daemonNotConfigured,
        message: s.daemonNotConfiguredHint,
        actionLabel: s.openSettings,
        onAction: widget.onOpenSettings,
        secondaryActionLabel: qrScanSupported ? s.scanToConnect : null,
        onSecondaryAction: qrScanSupported ? _scanToConnect : null,
      );
    }
    if (store.error != null && store.sessions.isEmpty) {
      return _CenteredHint(
        icon: Icons.cloud_off_outlined,
        title: s.cannotReachDaemon,
        message: '${store.error}',
        actionLabel: s.retry,
        onAction: () => store.refresh(),
      );
    }
    final groups = _filteredGroups(store);
    if (groups.isEmpty) {
      return _CenteredHint(
        icon: Icons.forum_outlined,
        title: _filter.isEmpty ? s.noSessionsYet : s.noMatches,
        message: _filter.isEmpty ? s.startSessionHint : s.tryDifferentFilter,
        actionLabel: _filter.isEmpty ? s.newSession : null,
        onAction: _filter.isEmpty ? widget.onNewSession : null,
      );
    }
    return RefreshIndicator(
      onRefresh: () => store.refresh(),
      child: ListView.builder(
        physics: const AlwaysScrollableScrollPhysics(),
        itemCount: groups.length,
        itemBuilder: (context, index) {
          final group = groups[index];
          return _WorkspaceGroupTile(
            group: group,
            selectedSessionId: widget.selectedSessionId,
            onOpenSession: widget.onOpenSession,
            onTogglePin: (session) =>
                store.setPinned(session.sessionId, !session.pinned),
            onDelete: (session) => _confirmDelete(session),
          );
        },
      ),
    );
  }

  List<WorkspaceSessionGroup> _filteredGroups(dynamic store) {
    final filter = _filter.trim().toLowerCase();
    final groups = store.workspaces.isNotEmpty
        ? store.workspaces as List<WorkspaceSessionGroup>
        : <WorkspaceSessionGroup>[
            WorkspaceSessionGroup(
              workspace: null,
              sessions: store.sessions as List<SessionSummary>,
            ),
          ];
    if (filter.isEmpty) return groups;
    return [
      for (final group in groups)
        WorkspaceSessionGroup(
          workspace: group.workspace,
          sessions: [
            for (final session in group.sessions)
              if ((session.title ?? '').toLowerCase().contains(filter) ||
                  session.sessionId.toLowerCase().contains(filter))
                session,
          ],
        ),
    ]..removeWhere((g) => g.sessions.isEmpty);
  }

  Future<void> _confirmDelete(SessionSummary session) async {
    final s = S.of(context);
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(s.deleteSessionTitle),
        content: Text(s.deleteSessionBody(session.title ?? session.sessionId)),
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
    if (confirmed == true) {
      await _app.sessionsStore.delete(session.sessionId);
    }
  }
}

class _WorkspaceGroupTile extends StatelessWidget {
  const _WorkspaceGroupTile({
    required this.group,
    required this.selectedSessionId,
    required this.onOpenSession,
    required this.onTogglePin,
    required this.onDelete,
  });

  final WorkspaceSessionGroup group;
  final String? selectedSessionId;
  final ValueChanged<String> onOpenSession;
  final ValueChanged<SessionSummary> onTogglePin;
  final ValueChanged<SessionSummary> onDelete;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 4),
          child: Row(
            children: [
              Icon(
                Icons.folder_outlined,
                size: 14,
                color: theme.colorScheme.outline,
              ),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  group.workspace ?? S.of(context).noWorkspace,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
            ],
          ),
        ),
        for (final session in group.sessions)
          _SessionTile(
            session: session,
            selected: session.sessionId == selectedSessionId,
            onTap: () => onOpenSession(session.sessionId),
            onTogglePin: () => onTogglePin(session),
            onDelete: () => onDelete(session),
          ),
      ],
    );
  }
}

class _SessionTile extends StatelessWidget {
  const _SessionTile({
    required this.session,
    required this.selected,
    required this.onTap,
    required this.onTogglePin,
    required this.onDelete,
  });

  final SessionSummary session;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onTogglePin;
  final VoidCallback onDelete;

  Color _statusColor(ThemeData theme) {
    switch (session.status) {
      case SessionStatus.running:
        return Colors.green;
      case SessionStatus.compacting:
        return Colors.orange;
      case SessionStatus.stopping:
      case SessionStatus.closing:
        return Colors.orange;
      case SessionStatus.offline:
      case SessionStatus.closed:
        return theme.colorScheme.outline;
      default:
        return theme.colorScheme.primary.withAlpha(160);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      color: selected ? theme.colorScheme.primaryContainer.withAlpha(90) : null,
      child: InkWell(
        onTap: onTap,
        onLongPress: () => _showActions(context),
        onSecondaryTap: () => _showActions(context),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
          child: Row(
            children: [
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _statusColor(theme),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        if (session.pinned)
                          Padding(
                            padding: const EdgeInsets.only(right: 4),
                            child: Icon(
                              Icons.push_pin,
                              size: 12,
                              color: theme.colorScheme.primary,
                            ),
                          ),
                        Expanded(
                          child: Text(
                            session.title ?? S.of(context).untitledSession,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: theme.textTheme.bodyMedium?.copyWith(
                              fontWeight: session.activeRunId != null
                                  ? FontWeight.w600
                                  : FontWeight.normal,
                            ),
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 2),
                    Text(
                      [
                        session.config.model,
                        if (session.messageCount != null)
                          S.of(context).messageCount(session.messageCount!),
                        if (session.queuedRuns > 0)
                          S.of(context).queuedCount(session.queuedRuns),
                      ].join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ],
                ),
              ),
              if (session.subagents.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(left: 6),
                  child: Icon(
                    Icons.account_tree_outlined,
                    size: 14,
                    color: theme.colorScheme.outline,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  void _showActions(BuildContext context) {
    final s = S.of(context);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: Icon(
                session.pinned ? Icons.push_pin_outlined : Icons.push_pin,
              ),
              title: Text(session.pinned ? s.unpin : s.pin),
              onTap: () {
                Navigator.of(context).pop();
                onTogglePin();
              },
            ),
            ListTile(
              leading: const Icon(Icons.copy_outlined),
              title: Text(s.copySessionId),
              onTap: () {
                Navigator.of(context).pop();
                Clipboard.setData(ClipboardData(text: session.sessionId));
              },
            ),
            ListTile(
              leading: Icon(
                Icons.delete_outline,
                color: Theme.of(context).colorScheme.error,
              ),
              title: Text(
                s.delete,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
              onTap: () {
                Navigator.of(context).pop();
                onDelete();
              },
            ),
          ],
        ),
      ),
    );
  }
}

class _CenteredHint extends StatelessWidget {
  const _CenteredHint({
    required this.icon,
    required this.title,
    required this.message,
    this.actionLabel,
    this.onAction,
    this.secondaryActionLabel,
    this.onSecondaryAction,
  });

  final IconData icon;
  final String title;
  final String message;
  final String? actionLabel;
  final VoidCallback? onAction;
  final String? secondaryActionLabel;
  final VoidCallback? onSecondaryAction;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 40, color: theme.colorScheme.outline),
            const SizedBox(height: 12),
            Text(title, style: theme.textTheme.titleSmall),
            const SizedBox(height: 6),
            Text(
              message,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            if (actionLabel != null) ...[
              const SizedBox(height: 12),
              FilledButton(onPressed: onAction, child: Text(actionLabel!)),
            ],
            if (secondaryActionLabel != null) ...[
              const SizedBox(height: 8),
              OutlinedButton(
                onPressed: onSecondaryAction,
                child: Text(secondaryActionLabel!),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
