import 'package:flutter/material.dart';

import '../app.dart';
import '../i18n/strings.dart';
import '../state/app_state.dart';
import '../state/session_controller.dart';
import 'pages/chat_page.dart';
import 'pages/scheduled_tasks_page.dart';
import 'pages/sessions_page.dart';
import 'pages/settings_page.dart';
import 'widgets/workspace_picker.dart';

/// Adaptive app shell: a sessions sidebar beside a detail pane on wide
/// screens (desktop / tablet landscape), plain stacked navigation on phones.
class HomeShell extends StatefulWidget {
  const HomeShell({super.key});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  /// Detail-pane selection on wide screens.
  String? _selectedSessionId;
  bool _creatingNew = false;
  NewSessionConfig? _newSessionConfig;
  bool _showingTasks = false;

  static const _wideBreakpoint = 860.0;

  AppState get _app => AppScope.of(context);

  bool _pollingStarted = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_pollingStarted) {
      _pollingStarted = true;
      _app.sessionsStore.startPolling();
    }
  }

  void _openSession(String sessionId, {required bool wide}) {
    if (wide) {
      setState(() {
        _selectedSessionId = sessionId;
        _creatingNew = false;
        _showingTasks = false;
      });
    } else {
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => ChatPage(target: AttachSessionTarget(sessionId)),
        ),
      );
    }
  }

  Future<void> _startNewSession({required bool wide}) async {
    final config = await showNewSessionDialog(context, _app);
    if (config == null || !mounted) return;
    if (wide) {
      setState(() {
        _creatingNew = true;
        _newSessionConfig = config;
        _selectedSessionId = null;
        _showingTasks = false;
      });
    } else {
      await Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => ChatPage(
            target: NewSessionTarget(
              profileId: config.profileId,
              workspace: config.workspace,
              capabilityMode: config.capabilityMode,
            ),
          ),
        ),
      );
    }
    _app.sessionsStore.refresh(silent: true);
  }

  void _openTasks({required bool wide}) {
    if (wide) {
      setState(() {
        _showingTasks = true;
        _creatingNew = false;
        _selectedSessionId = null;
      });
    } else {
      Navigator.of(context).push(
        MaterialPageRoute<void>(builder: (_) => const ScheduledTasksPage()),
      );
    }
  }

  void _openSettings() {
    Navigator.of(
      context,
    ).push(MaterialPageRoute<void>(builder: (_) => const SettingsPage()));
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _app,
      builder: (context, _) {
        return LayoutBuilder(
          builder: (context, constraints) {
            final wide = constraints.maxWidth >= _wideBreakpoint;
            if (!wide) {
              return SessionsPage(
                key: ValueKey(_app.client),
                embedded: false,
                selectedSessionId: null,
                onOpenSession: (id) => _openSession(id, wide: false),
                onNewSession: () => _startNewSession(wide: false),
                onOpenTasks: () => _openTasks(wide: false),
                onOpenSettings: _openSettings,
              );
            }
            return Scaffold(
              body: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  SizedBox(
                    width: 320,
                    child: SessionsPage(
                      key: ValueKey(_app.client),
                      embedded: true,
                      selectedSessionId: _selectedSessionId,
                      onOpenSession: (id) => _openSession(id, wide: true),
                      onNewSession: () => _startNewSession(wide: true),
                      onOpenTasks: () => _openTasks(wide: true),
                      onOpenSettings: _openSettings,
                    ),
                  ),
                  const VerticalDivider(width: 1),
                  Expanded(child: _buildDetailPane()),
                ],
              ),
            );
          },
        );
      },
    );
  }

  Widget _buildDetailPane() {
    if (_showingTasks) {
      return ScheduledTasksPage(
        embedded: true,
        onOpenSession: (id) => setState(() {
          _selectedSessionId = id;
          _showingTasks = false;
        }),
      );
    }
    if (_creatingNew && _newSessionConfig != null) {
      final config = _newSessionConfig!;
      return ChatPage(
        key: ValueKey(
          'new-${config.workspace}-${config.capabilityMode}-${config.profileId}',
        ),
        embedded: true,
        target: NewSessionTarget(
          profileId: config.profileId,
          workspace: config.workspace,
          capabilityMode: config.capabilityMode,
        ),
        onSessionCreated: (sessionId) {
          setState(() {
            _creatingNew = false;
            _selectedSessionId = sessionId;
          });
        },
      );
    }
    final selected = _selectedSessionId;
    if (selected != null) {
      return ChatPage(
        key: ValueKey('attach-$selected'),
        embedded: true,
        target: AttachSessionTarget(selected),
        onSessionDeleted: () => setState(() => _selectedSessionId = null),
        onForked: (newId) => setState(() => _selectedSessionId = newId),
      );
    }
    return const _EmptyDetail();
  }
}

class _EmptyDetail extends StatelessWidget {
  const _EmptyDetail();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.terminal_rounded,
              size: 56,
              color: theme.colorScheme.outline,
            ),
            const SizedBox(height: 16),
            Text(
              S.of(context).selectSessionHint,
              style: theme.textTheme.titleMedium?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
