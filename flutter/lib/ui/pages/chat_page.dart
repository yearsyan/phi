import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../app.dart';
import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';
import '../../state/session_controller.dart';
import '../widgets/composer.dart';
import '../widgets/timeline.dart';

/// Full chat view for one session target (new or attach). Used standalone on
/// phones (pushed route) and embedded as the detail pane on wide screens.
class ChatPage extends StatefulWidget {
  const ChatPage({
    super.key,
    required this.target,
    this.embedded = false,
    this.onSessionCreated,
    this.onSessionDeleted,
    this.onForked,
    this.controllerFactory,
  });

  final SessionTarget target;
  final bool embedded;

  /// Test hook: build the [SessionController] instead of constructing one
  /// from the app client.
  final SessionController Function()? controllerFactory;

  /// Called when a `/new` target's first prompt activates the session.
  final ValueChanged<String>? onSessionCreated;
  final VoidCallback? onSessionDeleted;
  final ValueChanged<String>? onForked;

  @override
  State<ChatPage> createState() => _ChatPageState();
}

class _ChatPageState extends State<ChatPage> {
  SessionController? _controller;
  AppState? _app;
  Object? _boundClient;
  final ScrollController _scroll = ScrollController();
  bool _stickToBottom = true;
  bool _userDragging = false;
  String? _lastCreatedSessionId;
  late SessionTarget _currentTarget = widget.target;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final app = AppScope.of(context);
    _app = app;
    if (!identical(app.client, _boundClient)) {
      _boundClient = app.client;
      _controller?.dispose();
      _controller = null;
      _createController();
    }
  }

  void _createController() {
    final app = _app!;
    final controller =
        widget.controllerFactory?.call() ??
        SessionController(
          client: app.client,
          target: _currentTarget,
          onSessionListMayChange: () {
            app.sessionsStore.refresh(silent: true);
            final sessionId = _controller?.sessionId;
            if (sessionId != null &&
                sessionId != _lastCreatedSessionId &&
                _currentTarget is NewSessionTarget) {
              _lastCreatedSessionId = sessionId;
              widget.onSessionCreated?.call(sessionId);
            }
          },
        );
    controller.addListener(_onControllerUpdate);
    _controller = controller;
    controller.start();
  }

  /// Pre-activation provider-profile switch: reconnect `/v1/ws/new` with the
  /// chosen profile (mirrors the web client).
  void _switchProfile(String profileId) {
    final target = _currentTarget;
    if (target is! NewSessionTarget || _controller?.sessionId != null) return;
    _controller?.dispose();
    setState(() {
      _currentTarget = NewSessionTarget(
        profileId: profileId,
        agentProfileId: target.agentProfileId,
        capabilityMode: target.capabilityMode,
        workspace: target.workspace,
      );
      _controller = null;
    });
    _createController();
  }

  void _onControllerUpdate() {
    // Never fight an active drag: while streaming, yanking to the bottom on
    // every delta makes it impossible to scroll up and read earlier output.
    if (_stickToBottom && !_userDragging && _scroll.hasClients) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients && !_userDragging) {
          _scroll.jumpTo(_scroll.position.maxScrollExtent);
        }
      });
    }
  }

  bool _handleScrollNotification(ScrollNotification notification) {
    if (notification is ScrollStartNotification &&
        notification.dragDetails != null) {
      _userDragging = true;
    } else if (notification is ScrollEndNotification) {
      _userDragging = false;
    }
    if (notification is ScrollUpdateNotification ||
        notification is ScrollEndNotification) {
      final position = _scroll.position;
      _stickToBottom = position.maxScrollExtent - position.pixels < 120;
    }
    return false;
  }

  @override
  void dispose() {
    _controller?.dispose();
    _scroll.dispose();
    super.dispose();
  }

  SessionController get controller => _controller!;

  Future<void> _fork(int historyIndex) async {
    final s = S.of(context);
    final sessionId = controller.sessionId;
    if (sessionId == null) return;
    final forkIndex = forkMessageIndex(historyIndex, controller.compactions);
    if (forkIndex == null) {
      _toast(s.forkPredatesCompaction);
      return;
    }
    try {
      final summary = await _app!.client.forkSession(sessionId, forkIndex);
      _app!.sessionsStore.refresh(silent: true);
      if (!mounted) return;
      _toast(s.forkedIntoNewSession);
      if (widget.onForked != null) {
        widget.onForked!(summary.sessionId);
      } else {
        await Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) =>
                ChatPage(target: AttachSessionTarget(summary.sessionId)),
          ),
        );
      }
    } catch (error) {
      _toast(s.forkFailed(error));
    }
  }

  Future<void> _deleteSession() async {
    final s = S.of(context);
    final sessionId = controller.sessionId;
    if (sessionId == null) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(s.deleteSessionTitle),
        content: Text(s.deleteSessionConfirmBody),
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
    if (confirmed != true || !mounted) return;
    try {
      await _app!.sessionsStore.delete(sessionId);
      if (!mounted) return;
      if (widget.onSessionDeleted != null) {
        widget.onSessionDeleted!();
      } else {
        Navigator.of(context).pop();
      }
    } catch (error) {
      _toast(S.of(context).deleteFailed(error));
    }
  }

  void _toast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final controller = _controller;
    return Scaffold(
      appBar: AppBar(
        automaticallyImplyLeading: !widget.embedded,
        title: controller == null
            ? Text(s.newSession)
            : ListenableBuilder(
                listenable: controller,
                builder: (context, _) => _HeaderTitle(controller: controller),
              ),
        actions: [
          if (controller != null)
            ListenableBuilder(
              listenable: controller,
              builder: (context, _) => Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _UsageIndicator(controller: controller),
                  _StatusChip(controller: controller),
                  _HeaderActions(
                    controller: controller,
                    onCompact: () => controller.compact(),
                    onDelete: controller.sessionId != null
                        ? _deleteSession
                        : null,
                    onCopyId: controller.sessionId != null
                        ? () {
                            Clipboard.setData(
                              ClipboardData(text: controller.sessionId!),
                            );
                            _toast(S.of(context).sessionIdCopied);
                          }
                        : null,
                    onReconnect: controller.retry,
                  ),
                ],
              ),
            ),
        ],
      ),
      body: controller == null
          ? const Center(child: CircularProgressIndicator())
          : ListenableBuilder(
              listenable: controller,
              builder: (context, _) => Column(
                children: [
                  _NoticeBar(controller: controller),
                  _ConnectionBar(controller: controller),
                  Expanded(
                    child: NotificationListener<ScrollNotification>(
                      onNotification: _handleScrollNotification,
                      child: ChatTimeline(
                        controller: controller,
                        scrollController: _scroll,
                        onFork: _fork,
                      ),
                    ),
                  ),
                  Composer(
                    controller: controller,
                    client: _app!.client,
                    onSwitchProfile: _switchProfile,
                  ),
                ],
              ),
            ),
    );
  }
}

class _HeaderTitle extends StatelessWidget {
  const _HeaderTitle({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final title =
        controller.title ??
        (controller.sessionId == null ? s.newSession : s.untitledSession);
    final subtitle = controller.workspace;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.titleMedium,
        ),
        if (subtitle != null)
          Text(
            subtitle,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
      ],
    );
  }
}

class _HeaderActions extends StatelessWidget {
  const _HeaderActions({
    required this.controller,
    required this.onCompact,
    required this.onDelete,
    required this.onCopyId,
    required this.onReconnect,
  });

  final SessionController controller;
  final VoidCallback onCompact;
  final VoidCallback? onDelete;
  final VoidCallback? onCopyId;
  final VoidCallback onReconnect;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    return PopupMenuButton<String>(
      tooltip: s.sessionActions,
      onSelected: (value) {
        switch (value) {
          case 'compact':
            onCompact();
          case 'copy_id':
            onCopyId?.call();
          case 'reconnect':
            onReconnect();
          case 'delete':
            onDelete?.call();
        }
      },
      itemBuilder: (context) => [
        PopupMenuItem(
          value: 'compact',
          child: ListTile(
            dense: true,
            leading: const Icon(Icons.compress_outlined),
            title: Text(s.compactContext),
            contentPadding: EdgeInsets.zero,
          ),
        ),
        if (onCopyId != null)
          PopupMenuItem(
            value: 'copy_id',
            child: ListTile(
              dense: true,
              leading: const Icon(Icons.copy_outlined),
              title: Text(s.copySessionId),
              contentPadding: EdgeInsets.zero,
            ),
          ),
        PopupMenuItem(
          value: 'reconnect',
          child: ListTile(
            dense: true,
            leading: const Icon(Icons.refresh_rounded),
            title: Text(s.reconnect),
            contentPadding: EdgeInsets.zero,
          ),
        ),
        if (onDelete != null)
          PopupMenuItem(
            value: 'delete',
            child: ListTile(
              dense: true,
              leading: Icon(
                Icons.delete_outline,
                color: theme.colorScheme.error,
              ),
              title: Text(
                s.deleteSession,
                style: TextStyle(color: theme.colorScheme.error),
              ),
              contentPadding: EdgeInsets.zero,
            ),
          ),
      ],
    );
  }
}

class _StatusChip extends StatelessWidget {
  const _StatusChip({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final (label, color) = switch (controller.status) {
      SessionStatus.running => (s.statusRunning, Colors.green),
      SessionStatus.compacting => (s.statusCompacting, Colors.orange),
      SessionStatus.stopping => (s.statusStopping, Colors.orange),
      SessionStatus.offline => (s.statusOffline, theme.colorScheme.outline),
      SessionStatus.closed => (s.statusClosed, theme.colorScheme.outline),
      SessionStatus.awaitingFirstPrompt => (
        s.statusReady,
        theme.colorScheme.primary,
      ),
      _ => (s.statusIdle, theme.colorScheme.outline),
    };
    return Padding(
      padding: const EdgeInsets.only(right: 4),
      child: Chip(
        visualDensity: VisualDensity.compact,
        padding: EdgeInsets.zero,
        labelPadding: const EdgeInsets.symmetric(horizontal: 6),
        avatar: Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(shape: BoxShape.circle, color: color),
        ),
        label: Text(
          controller.queuedRuns > 0
              ? '$label · ${s.queuedSuffix(controller.queuedRuns)}'
              : label,
          style: theme.textTheme.labelSmall,
        ),
      ),
    );
  }
}

/// Context-usage ring in the header; tap for a detailed breakdown.
class _UsageIndicator extends StatelessWidget {
  const _UsageIndicator({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    final usage = controller.contextUsage;
    if (usage == null || usage.maxTokens == 0) {
      return const SizedBox.shrink();
    }
    final theme = Theme.of(context);
    final s = S.of(context);
    final fraction = usage.fraction.clamp(0.0, 1.0);
    final color = fraction > 0.85
        ? theme.colorScheme.error
        : theme.colorScheme.primary;
    return Tooltip(
      message: s.contextTokens(
        _compact(usage.usedTokens),
        _compact(usage.maxTokens),
      ),
      child: InkWell(
        onTap: () => _showDetails(context),
        borderRadius: BorderRadius.circular(16),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(
                  value: fraction,
                  strokeWidth: 2.4,
                  backgroundColor: theme.colorScheme.outlineVariant.withAlpha(
                    80,
                  ),
                  color: color,
                ),
              ),
              const SizedBox(width: 4),
              Text(
                '${(fraction * 100).round()}%',
                style: theme.textTheme.labelSmall?.copyWith(color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }

  void _showDetails(BuildContext context) {
    final s = S.of(context);
    final usage = controller.usage;
    final contextUsage = controller.contextUsage;
    showDialog<void>(
      context: context,
      builder: (context) {
        final theme = Theme.of(context);
        return AlertDialog(
          title: Text(s.contextUsageTitle),
          content: SizedBox(
            width: 320,
            child: usage == null && contextUsage == null
                ? Text(s.noUsageYet)
                : Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (contextUsage != null) ...[
                        _meter(context),
                        const SizedBox(height: 12),
                        _row(
                          context,
                          s.usedTokens,
                          _compact(contextUsage.usedTokens),
                        ),
                        _row(
                          context,
                          s.remainingTokens,
                          _compact(contextUsage.remainingTokens),
                        ),
                        _row(
                          context,
                          s.maxTokens,
                          _compact(contextUsage.maxTokens),
                        ),
                      ],
                      if (usage?.last != null) ...[
                        const Divider(height: 20),
                        _section(theme, s.lastCallTokens),
                        _row(
                          context,
                          s.inputTokens,
                          _compact(usage!.last!.inputTokens),
                        ),
                        _row(
                          context,
                          s.outputTokens,
                          _compact(usage.last!.outputTokens),
                        ),
                        if (usage.last!.cachedInputTokens > 0)
                          _row(
                            context,
                            s.cachedTokens,
                            _compact(usage.last!.cachedInputTokens),
                          ),
                      ],
                      if (usage != null) ...[
                        const Divider(height: 20),
                        _section(theme, s.cumulativeTokens),
                        _row(
                          context,
                          s.inputTokens,
                          _compact(usage.cumulative.inputTokens),
                        ),
                        _row(
                          context,
                          s.outputTokens,
                          _compact(usage.cumulative.outputTokens),
                        ),
                        if (usage.cumulative.cachedInputTokens > 0)
                          _row(
                            context,
                            s.cachedTokens,
                            _compact(usage.cumulative.cachedInputTokens),
                          ),
                      ],
                    ],
                  ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(),
              child: Text(s.close),
            ),
          ],
        );
      },
    );
  }

  Widget _meter(BuildContext context) {
    final usage = controller.contextUsage!;
    final fraction = usage.fraction.clamp(0.0, 1.0);
    final theme = Theme.of(context);
    return ClipRRect(
      borderRadius: BorderRadius.circular(4),
      child: LinearProgressIndicator(
        value: fraction,
        minHeight: 8,
        backgroundColor: theme.colorScheme.outlineVariant.withAlpha(80),
        color: fraction > 0.85
            ? theme.colorScheme.error
            : theme.colorScheme.primary,
      ),
    );
  }

  Widget _section(ThemeData theme, String label) {
    return Align(
      alignment: Alignment.centerLeft,
      child: Text(
        label,
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.outline,
        ),
      ),
    );
  }

  Widget _row(BuildContext context, String label, String value) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text(label, style: theme.textTheme.bodySmall),
          Text(
            value,
            style: theme.textTheme.bodySmall?.copyWith(
              fontFamily: 'Menlo',
              fontFamilyFallback: const ['monospace'],
            ),
          ),
        ],
      ),
    );
  }

  static String _compact(int value) {
    if (value >= 1000000) return '${(value / 1000000).toStringAsFixed(1)}M';
    if (value >= 1000) return '${(value / 1000).toStringAsFixed(1)}k';
    return '$value';
  }
}

class _NoticeBar extends StatelessWidget {
  const _NoticeBar({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    if (controller.notices.isEmpty) return const SizedBox.shrink();
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.errorContainer.withAlpha(120),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < controller.notices.length; i++)
            ListTile(
              dense: true,
              leading: Icon(
                Icons.warning_amber_rounded,
                size: 18,
                color: theme.colorScheme.error,
              ),
              title: Text(
                controller.notices[i],
                style: theme.textTheme.bodySmall,
              ),
              trailing: IconButton(
                icon: const Icon(Icons.close, size: 16),
                onPressed: () => controller.clearNotice(i),
              ),
            ),
        ],
      ),
    );
  }
}

class _ConnectionBar extends StatelessWidget {
  const _ConnectionBar({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    switch (controller.phase) {
      case SessionConnectionPhase.ready:
        return const SizedBox.shrink();
      case SessionConnectionPhase.connecting:
      case SessionConnectionPhase.preparing:
        return const LinearProgressIndicator(minHeight: 2);
      case SessionConnectionPhase.reconnecting:
        return _Bar(
          icon: Icons.sync_problem_rounded,
          text: controller.connectionError != null
              ? s.reconnectingWithError(controller.connectionError!)
              : s.reconnecting,
          color: theme.colorScheme.tertiaryContainer,
        );
      case SessionConnectionPhase.error:
        return _Bar(
          icon: Icons.error_outline,
          text:
              controller.connectionError ??
              controller.fatalError?.message ??
              s.connectionFailed,
          color: theme.colorScheme.errorContainer,
          action: TextButton(onPressed: controller.retry, child: Text(s.retry)),
        );
      case SessionConnectionPhase.idle:
        return const SizedBox.shrink();
    }
  }
}

class _Bar extends StatelessWidget {
  const _Bar({
    required this.icon,
    required this.text,
    required this.color,
    this.action,
  });

  final IconData icon;
  final String text;
  final Color color;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      color: color.withAlpha(110),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
        child: Row(
          children: [
            Icon(icon, size: 16),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                text,
                style: theme.textTheme.bodySmall,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            ?action,
          ],
        ),
      ),
    );
  }
}
