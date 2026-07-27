import 'package:flutter/material.dart';

import '../../app.dart';
import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../state/app_state.dart';
import '../../state/session_controller.dart';
import '../widgets/workspace_picker.dart';
import 'chat_page.dart';

/// Scheduled-task CRUD: list, create, edit, enable/disable, run now, delete.
class ScheduledTasksPage extends StatefulWidget {
  const ScheduledTasksPage({
    super.key,
    this.embedded = false,
    this.onOpenSession,
  });

  final bool embedded;
  final ValueChanged<String>? onOpenSession;

  @override
  State<ScheduledTasksPage> createState() => _ScheduledTasksPageState();
}

class _ScheduledTasksPageState extends State<ScheduledTasksPage> {
  List<ScheduledTask>? _tasks;
  Object? _error;
  bool _loading = true;

  AppState get _app => AppScope.of(context);

  bool _initialLoadDone = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_initialLoadDone) {
      _initialLoadDone = true;
      _load();
    }
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final tasks = await _app.client.listScheduledTasks();
      if (!mounted) return;
      setState(() {
        _tasks = tasks;
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

  void _openSession(String sessionId) {
    if (widget.onOpenSession != null) {
      widget.onOpenSession!(sessionId);
    } else {
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => ChatPage(target: AttachSessionTarget(sessionId)),
        ),
      );
    }
  }

  Future<void> _toggle(ScheduledTask task, bool enabled) async {
    try {
      await _app.client.setScheduledTaskEnabled(
        task.taskId,
        enabled,
        task.revision,
      );
      _load();
    } catch (error) {
      _toast('$error');
    }
  }

  Future<void> _runNow(ScheduledTask task) async {
    final s = S.of(context);
    try {
      await _app.client.runScheduledTaskNow(task.taskId);
      if (!mounted) return;
      _toast(s.runStarted);
      _load();
    } catch (error) {
      _toast('$error');
    }
  }

  Future<void> _delete(ScheduledTask task) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(S.of(context).deleteTaskTitle(task.name)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(S.of(context).cancel),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: Theme.of(context).colorScheme.error,
            ),
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(S.of(context).delete),
          ),
        ],
      ),
    );
    if (confirmed == true) {
      try {
        await _app.client.deleteScheduledTask(task.taskId);
        _load();
      } catch (error) {
        _toast('$error');
      }
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
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(
        automaticallyImplyLeading: !widget.embedded,
        title: Text(S.of(context).scheduledTasks),
        actions: [
          IconButton(icon: const Icon(Icons.refresh_rounded), onPressed: _load),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _showTaskDialog(),
        icon: const Icon(Icons.add),
        label: Text(S.of(context).newTask),
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : _error != null
          ? Center(child: Text('$_error'))
          : _tasks == null || _tasks!.isEmpty
          ? Center(
              child: Text(
                S.of(context).noScheduledTasks,
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: theme.colorScheme.outline,
                ),
              ),
            )
          : RefreshIndicator(
              onRefresh: _load,
              child: ListView.builder(
                padding: const EdgeInsets.only(bottom: 96),
                itemCount: _tasks!.length,
                itemBuilder: (context, index) => _buildTaskTile(_tasks![index]),
              ),
            ),
    );
  }

  Widget _buildTaskTile(ScheduledTask task) {
    final theme = Theme.of(context);
    final lastRun = task.lastRun;
    final outcomeColor = switch (lastRun?.outcome) {
      'succeeded' => Colors.green,
      'failed' => theme.colorScheme.error,
      'running' => Colors.blue,
      null => theme.colorScheme.outline,
      _ => Colors.orange,
    };
    return Card(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 10, 6, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    task.name,
                    style: theme.textTheme.titleSmall,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                Switch(
                  value: task.enabled,
                  onChanged: (value) => _toggle(task, value),
                ),
                PopupMenuButton<String>(
                  onSelected: (value) {
                    switch (value) {
                      case 'edit':
                        _showTaskDialog(task);
                      case 'run':
                        _runNow(task);
                      case 'delete':
                        _delete(task);
                      case 'open_session':
                        final sessionId = lastRun?.sessionId;
                        if (sessionId != null) _openSession(sessionId);
                    }
                  },
                  itemBuilder: (context) => [
                    PopupMenuItem(
                      value: 'edit',
                      child: Text(S.of(context).editTask),
                    ),
                    PopupMenuItem(
                      value: 'run',
                      child: Text(S.of(context).runNow),
                    ),
                    if (lastRun?.sessionId != null)
                      PopupMenuItem(
                        value: 'open_session',
                        child: Text(S.of(context).openLastRun),
                      ),
                    PopupMenuItem(
                      value: 'delete',
                      child: Text(
                        S.of(context).delete,
                        style: TextStyle(color: theme.colorScheme.error),
                      ),
                    ),
                  ],
                ),
              ],
            ),
            Text(
              task.prompt,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: 6),
            LayoutBuilder(
              builder: (context, constraints) => Wrap(
                spacing: 12,
                runSpacing: 4,
                children: [
                  _meta(
                    theme,
                    Icons.schedule,
                    _describeSchedule(task.schedule),
                    constraints.maxWidth,
                  ),
                  if (task.workspace != null)
                    _meta(
                      theme,
                      Icons.folder_outlined,
                      task.workspace!,
                      constraints.maxWidth,
                    ),
                  if (task.outputChannelId != null)
                    _meta(
                      theme,
                      Icons.send_rounded,
                      S.of(context).outputChannelValue(task.outputChannelId!),
                      constraints.maxWidth,
                    ),
                  if (task.nextRunAt != null && task.enabled)
                    _meta(
                      theme,
                      Icons.event_outlined,
                      S.of(context).nextRun(_formatTime(task.nextRunAt!)),
                      constraints.maxWidth,
                    ),
                  if (lastRun != null)
                    _meta(
                      theme,
                      Icons.history,
                      lastRun.outcome,
                      constraints.maxWidth,
                      color: outcomeColor,
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _meta(
    ThemeData theme,
    IconData icon,
    String text,
    double maxWidth, {
    Color? color,
  }) {
    final effective = color ?? theme.colorScheme.outline;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 12, color: effective),
        const SizedBox(width: 4),
        ConstrainedBox(
          constraints: BoxConstraints(
            // Icon + gap so the whole row stays inside the tile.
            maxWidth: maxWidth > 16 ? maxWidth - 16 : 0,
          ),
          child: Text(
            text,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.labelSmall?.copyWith(color: effective),
          ),
        ),
      ],
    );
  }

  String _describeSchedule(ScheduledTaskSchedule schedule) {
    final s = S.of(context);
    if (schedule.type == 'daily') {
      final days = schedule.weekdays.length == 7
          ? s.everyDay
          : schedule.weekdays.map(s.weekdayShort).join(' ');
      return s.dailyScheduleValue(
        schedule.time ?? '',
        days,
        schedule.timezone ?? '',
      );
    }
    final unit = switch (schedule.unit) {
      'minutes' => s.minutesLabel,
      'days' => s.daysLabel,
      _ => s.hoursLabel,
    };
    return '${s.everyLabel} ${schedule.every} $unit';
  }

  static String _formatTime(String iso) {
    final parsed = DateTime.tryParse(iso);
    if (parsed == null) return iso;
    final local = parsed.toLocal();
    String two(int v) => v.toString().padLeft(2, '0');
    return '${local.month}/${local.day} ${two(local.hour)}:${two(local.minute)}';
  }

  Future<void> _showTaskDialog([ScheduledTask? task]) async {
    final saved = await showDialog<bool>(
      context: context,
      builder: (context) => _TaskEditorDialog(app: _app, task: task),
    );
    if (saved == true) _load();
  }
}

/* ------------------------------------------------------------------------- */
/* Task editor dialog                                                        */
/* ------------------------------------------------------------------------- */

class _TaskEditorDialog extends StatefulWidget {
  const _TaskEditorDialog({required this.app, this.task});

  final AppState app;
  final ScheduledTask? task;

  @override
  State<_TaskEditorDialog> createState() => _TaskEditorDialogState();
}

class _TaskEditorDialogState extends State<_TaskEditorDialog> {
  late final TextEditingController _name;
  late final TextEditingController _prompt;
  late final TextEditingController _timezone;
  late final TextEditingController _every;
  late String? _workspace;
  late String _kind;
  late String _unit;
  List<PublicOutputChannel> _outputChannels = const [];
  late String _outputChannelId;
  late TimeOfDay _time;
  late Set<String> _weekdays;
  bool _submitting = false;
  bool _loadingOutputChannels = true;
  String? _error;

  static const _weekdayOrder = [
    'monday',
    'tuesday',
    'wednesday',
    'thursday',
    'friday',
    'saturday',
    'sunday',
  ];

  static TimeOfDay? _parseTime(String? value) {
    final parts = value?.split(':');
    if (parts == null || parts.length != 2) return null;
    final hour = int.tryParse(parts[0]);
    final minute = int.tryParse(parts[1]);
    if (hour == null ||
        minute == null ||
        hour < 0 ||
        hour > 23 ||
        minute < 0 ||
        minute > 59) {
      return null;
    }
    return TimeOfDay(hour: hour, minute: minute);
  }

  @override
  void initState() {
    super.initState();
    final task = widget.task;
    final schedule = task?.schedule;
    _name = TextEditingController(text: task?.name ?? '');
    _prompt = TextEditingController(text: task?.prompt ?? '');
    _workspace = task?.workspace;
    _kind = schedule?.type ?? 'interval';
    _unit = schedule?.unit ?? 'hours';
    _outputChannelId = task?.outputChannelId ?? '';
    _time = _parseTime(schedule?.time) ?? const TimeOfDay(hour: 9, minute: 0);
    _timezone = TextEditingController(text: schedule?.timezone ?? 'UTC');
    _every = TextEditingController(text: (schedule?.every ?? 1).toString());
    _weekdays = schedule?.type == 'daily'
        ? schedule!.weekdays.toSet()
        : {'monday', 'tuesday', 'wednesday', 'thursday', 'friday'};
    _loadOutputChannels();
  }

  Future<void> _loadOutputChannels() async {
    try {
      final channels = await widget.app.client.listOutputChannels();
      if (!mounted) return;
      setState(() {
        _outputChannels = channels;
        _loadingOutputChannels = false;
      });
    } on DaemonError catch (error) {
      if (!mounted) return;
      setState(() {
        _loadingOutputChannels = false;
        if (error.statusCode != 404 &&
            error.code != 'output_channels_unavailable') {
          _error = S.of(context).outputChannelsLoadFailed(error);
        }
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loadingOutputChannels = false;
        _error = S.of(context).outputChannelsLoadFailed(error);
      });
    }
  }

  @override
  void dispose() {
    _name.dispose();
    _prompt.dispose();
    _timezone.dispose();
    _every.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_name.text.trim().isEmpty || _prompt.text.trim().isEmpty) {
      setState(() => _error = S.of(context).namePromptRequired);
      return;
    }
    setState(() {
      _submitting = true;
      _error = null;
    });
    try {
      final schedule = _kind == 'daily'
          ? ScheduledTaskSchedule.daily(
              time:
                  '${_time.hour.toString().padLeft(2, '0')}:${_time.minute.toString().padLeft(2, '0')}',
              weekdays: _weekdayOrder.where(_weekdays.contains).toList(),
              timezone: _timezone.text.trim().isEmpty
                  ? 'UTC'
                  : _timezone.text.trim(),
            )
          : ScheduledTaskSchedule.interval(
              every: int.tryParse(_every.text.trim()) ?? 1,
              unit: _unit,
            );
      final task = widget.task;
      if (task == null) {
        await widget.app.client.createScheduledTask(
          name: _name.text.trim(),
          prompt: _prompt.text.trim(),
          workspace: _workspace,
          outputChannelId: _outputChannelId.isEmpty ? null : _outputChannelId,
          schedule: schedule,
        );
      } else {
        final workspace =
            _workspace ?? (await widget.app.client.browseWorkspace()).path;
        await widget.app.client.replaceScheduledTask(
          taskId: task.taskId,
          name: _name.text.trim(),
          prompt: _prompt.text.trim(),
          workspace: workspace,
          profileId: task.profileId ?? 'default',
          agentProfileId: task.agentProfileId ?? 'default',
          capabilityMode: task.capabilityMode,
          outputChannelId: _outputChannelId.isEmpty ? null : _outputChannelId,
          schedule: schedule,
          expectedRevision: task.revision,
        );
      }
      if (mounted) Navigator.of(context).pop(true);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _submitting = false;
        _error = '$error';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final editing = widget.task != null;
    final outputChannelIds = {
      if (_outputChannelId.isNotEmpty) _outputChannelId,
      ..._outputChannels.map((channel) => channel.outputChannelId),
    };
    return AlertDialog(
      title: Text(editing ? S.of(context).editTask : S.of(context).newTask),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              TextField(
                controller: _name,
                decoration: InputDecoration(
                  labelText: S.of(context).nameLabel,
                  border: const OutlineInputBorder(),
                  isDense: true,
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _prompt,
                minLines: 3,
                maxLines: 6,
                decoration: InputDecoration(
                  labelText: S.of(context).promptLabel,
                  alignLabelWithHint: true,
                  border: const OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              Row(
                children: [
                  Expanded(
                    child: Text(
                      _workspace ?? S.of(context).noWorkspace,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ),
                  TextButton.icon(
                    onPressed: () async {
                      final picked = await showWorkspaceBrowser(
                        context,
                        widget.app.client,
                        initialPath: _workspace,
                      );
                      if (picked != null) setState(() => _workspace = picked);
                    },
                    icon: const Icon(Icons.folder_open, size: 18),
                    label: Text(S.of(context).workspaceLabel),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              DropdownButtonFormField<String>(
                initialValue: _outputChannelId,
                decoration: InputDecoration(
                  labelText: S.of(context).outputChannel,
                  border: const OutlineInputBorder(),
                  isDense: true,
                ),
                items: [
                  DropdownMenuItem(
                    value: '',
                    child: Text(S.of(context).noOutputChannel),
                  ),
                  for (final outputChannelId in outputChannelIds)
                    DropdownMenuItem(
                      value: outputChannelId,
                      child: Text(
                        '$outputChannelId · Telegram',
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: _loadingOutputChannels || _submitting
                    ? null
                    : (value) => setState(() => _outputChannelId = value ?? ''),
              ),
              const SizedBox(height: 12),
              SegmentedButton<String>(
                segments: [
                  ButtonSegment(
                    value: 'interval',
                    label: Text(S.of(context).intervalLabel),
                  ),
                  ButtonSegment(
                    value: 'daily',
                    label: Text(S.of(context).dailyLabel),
                  ),
                ],
                selected: {_kind},
                onSelectionChanged: (value) =>
                    setState(() => _kind = value.first),
              ),
              const SizedBox(height: 12),
              if (_kind == 'interval')
                Row(
                  children: [
                    Text(S.of(context).everyLabel),
                    const SizedBox(width: 8),
                    SizedBox(
                      width: 64,
                      child: TextField(
                        controller: _every,
                        keyboardType: TextInputType.number,
                        decoration: const InputDecoration(
                          border: OutlineInputBorder(),
                          isDense: true,
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    DropdownButton<String>(
                      value: _unit,
                      items: [
                        DropdownMenuItem(
                          value: 'minutes',
                          child: Text(S.of(context).minutesLabel),
                        ),
                        DropdownMenuItem(
                          value: 'hours',
                          child: Text(S.of(context).hoursLabel),
                        ),
                        DropdownMenuItem(
                          value: 'days',
                          child: Text(S.of(context).daysLabel),
                        ),
                      ],
                      onChanged: (value) =>
                          setState(() => _unit = value ?? 'hours'),
                    ),
                  ],
                )
              else ...[
                Row(
                  children: [
                    TextButton.icon(
                      onPressed: () async {
                        final picked = await showTimePicker(
                          context: context,
                          initialTime: _time,
                        );
                        if (picked != null) setState(() => _time = picked);
                      },
                      icon: const Icon(Icons.access_time, size: 18),
                      label: Text(_time.format(context)),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: TextField(
                        controller: _timezone,
                        decoration: InputDecoration(
                          labelText: S.of(context).timezoneLabel,
                          border: const OutlineInputBorder(),
                          isDense: true,
                        ),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  children: [
                    for (final day in _weekdayOrder)
                      FilterChip(
                        label: Text(
                          S.of(context).weekdayShort(day),
                          style: const TextStyle(fontSize: 11),
                        ),
                        selected: _weekdays.contains(day),
                        onSelected: (selected) {
                          setState(() {
                            if (selected) {
                              _weekdays.add(day);
                            } else {
                              _weekdays.remove(day);
                            }
                          });
                        },
                      ),
                  ],
                ),
              ],
              if (_error != null) ...[
                const SizedBox(height: 8),
                Text(
                  _error!,
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.error,
                    fontSize: 12,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: Text(S.of(context).cancel),
        ),
        FilledButton(
          onPressed: _submitting ? null : _submit,
          child: _submitting
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : Text(editing ? S.of(context).save : S.of(context).create),
        ),
      ],
    );
  }
}
