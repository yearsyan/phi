import 'package:flutter/material.dart';

import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../platform/image_attachment_picker.dart';
import '../../state/daemon_client.dart';
import '../../state/session_controller.dart';

/// Chat input bar with capability/model/reasoning controls, slash-command
/// palette, and stop support.
class Composer extends StatefulWidget {
  const Composer({
    super.key,
    required this.controller,
    required this.client,
    this.onSwitchProfile,
  });

  final SessionController controller;
  final DaemonClient client;

  /// Called pre-activation (prepared `/new` session) to reconnect with a
  /// different provider profile.
  final ValueChanged<String>? onSwitchProfile;

  @override
  State<Composer> createState() => _ComposerState();
}

class _ComposerState extends State<Composer> {
  static const _maxAttachments = 3;

  final TextEditingController _text = TextEditingController();
  final FocusNode _focus = FocusNode();
  final List<PickedImageAttachment> _attachments = [];
  bool _pickingImages = false;

  @override
  void initState() {
    super.initState();
    _text.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  SessionController get _session => widget.controller;

  List<_SlashCommand> get _commands => [
    _SlashCommand(
      name: 'compact',
      description: S.of(context).compactDescription,
    ),
    for (final skill in _session.skills.where((s) => s.userInvocable))
      _SlashCommand(
        name: skill.name,
        description: skill.description,
        isSkill: true,
      ),
  ];

  List<_SlashCommand> get _matchingCommands {
    final text = _text.text;
    if (!text.startsWith('/')) return const [];
    final head = text.split(' ').first.substring(1).toLowerCase();
    if (text.contains(' ') && head.isNotEmpty) return const [];
    return _commands
        .where((c) => c.name.toLowerCase().startsWith(head))
        .take(6)
        .toList();
  }

  void _send() {
    final raw = _text.text;
    if (raw.trim().isEmpty && _attachments.isEmpty) return;

    // Parse leading slash command.
    if (_attachments.isEmpty && raw.trim().startsWith('/')) {
      final trimmed = raw.trim();
      final space = trimmed.indexOf(' ');
      final name = (space < 0 ? trimmed : trimmed.substring(0, space))
          .substring(1);
      final rest = space < 0 ? '' : trimmed.substring(space + 1);
      if (name == 'compact') {
        if (_session.compact(instructions: rest.isEmpty ? null : rest)) {
          _clearDraft();
        }
        return;
      }
      final skill = _session.skills
          .where((s) => s.userInvocable && s.name == name)
          .firstOrNull;
      if (skill != null) {
        if (_session.sendPrompt(rest, skillName: skill.name)) {
          _clearDraft();
        }
        return;
      }
    }

    if (_attachments.isEmpty) {
      if (_session.sendPrompt(raw)) _clearDraft();
      return;
    }

    final text = raw.trim();
    final content = Content.parts([
      if (text.isNotEmpty) ContentPart.text(text),
      for (final attachment in _attachments)
        ContentPart.imageUrl(attachment.dataUrl, detail: 'auto'),
    ]);
    if (_session.sendPromptContent(content)) {
      _clearDraft();
    }
  }

  void _clearDraft() {
    _text.clear();
    if (_attachments.isNotEmpty) {
      setState(_attachments.clear);
    }
  }

  Future<void> _pickImages() async {
    final s = S.of(context);
    final remaining = _maxAttachments - _attachments.length;
    if (remaining <= 0) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(s.imageLimitReached(_maxAttachments))),
      );
      return;
    }
    setState(() => _pickingImages = true);
    try {
      final picked = await ImageAttachmentPicker.pickImages(
        maxCount: remaining,
      );
      if (!mounted || picked.isEmpty) return;
      setState(() => _attachments.addAll(picked.take(remaining)));
    } catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(s.imagePickerFailed(error))));
    } finally {
      if (mounted) setState(() => _pickingImages = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final matching = _matchingCommands;
    final busy = _session.isBusy;
    final hasDraft = _text.text.trim().isNotEmpty || _attachments.isNotEmpty;
    final canSend = _session.canSend && hasDraft;

    return SafeArea(
      top: false,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 6, 12, 10),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (matching.isNotEmpty)
              Container(
                margin: const EdgeInsets.only(bottom: 6),
                constraints: const BoxConstraints(maxHeight: 220),
                decoration: BoxDecoration(
                  color: theme.colorScheme.surfaceContainerHigh,
                  borderRadius: BorderRadius.circular(10),
                  border: Border.all(color: theme.colorScheme.outlineVariant),
                ),
                child: ListView.builder(
                  shrinkWrap: true,
                  itemCount: matching.length,
                  itemBuilder: (context, index) {
                    final command = matching[index];
                    return ListTile(
                      dense: true,
                      leading: Icon(
                        command.isSkill
                            ? Icons.auto_awesome_outlined
                            : Icons.compress_outlined,
                        size: 18,
                      ),
                      title: Text('/${command.name}'),
                      subtitle: Text(
                        command.description,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      onTap: () {
                        _text.text = '/${command.name} ';
                        _text.selection = TextSelection.collapsed(
                          offset: _text.text.length,
                        );
                        _focus.requestFocus();
                      },
                    );
                  },
                ),
              ),
            Container(
              key: const ValueKey('composer-card'),
              decoration: BoxDecoration(
                color: theme.colorScheme.surface,
                borderRadius: BorderRadius.circular(30),
                border: Border.all(
                  color: theme.colorScheme.outlineVariant.withValues(
                    alpha: theme.brightness == Brightness.dark ? 0.72 : 0.38,
                  ),
                ),
                boxShadow: [
                  BoxShadow(
                    color: Colors.black.withValues(
                      alpha: theme.brightness == Brightness.dark ? 0.30 : 0.08,
                    ),
                    blurRadius: 24,
                    spreadRadius: -4,
                    offset: const Offset(0, 10),
                  ),
                ],
              ),
              clipBehavior: Clip.antiAlias,
              child: Material(
                color: Colors.transparent,
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(18, 14, 12, 10),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (_attachments.isNotEmpty) ...[
                        SizedBox(
                          key: const ValueKey('composer-attachment-preview'),
                          height: 62,
                          child: ListView.separated(
                            scrollDirection: Axis.horizontal,
                            itemCount: _attachments.length,
                            separatorBuilder: (_, _) =>
                                const SizedBox(width: 8),
                            itemBuilder: (context, index) {
                              final attachment = _attachments[index];
                              return _AttachmentPreview(
                                attachment: attachment,
                                onRemove: () => setState(
                                  () => _attachments.removeAt(index),
                                ),
                              );
                            },
                          ),
                        ),
                        const SizedBox(height: 8),
                      ],
                      TextField(
                        key: const ValueKey('composer-text-field'),
                        controller: _text,
                        focusNode: _focus,
                        enabled: _session.phase == SessionConnectionPhase.ready,
                        minLines: 1,
                        maxLines: 6,
                        textInputAction: TextInputAction.newline,
                        cursorColor: theme.colorScheme.primary,
                        style: theme.textTheme.bodyLarge?.copyWith(
                          fontSize: 17,
                          height: 1.38,
                        ),
                        decoration: InputDecoration(
                          border: InputBorder.none,
                          enabledBorder: InputBorder.none,
                          focusedBorder: InputBorder.none,
                          disabledBorder: InputBorder.none,
                          errorBorder: InputBorder.none,
                          focusedErrorBorder: InputBorder.none,
                          isDense: true,
                          contentPadding: EdgeInsets.zero,
                          hintText:
                              _session.phase == SessionConnectionPhase.ready
                              ? (busy ? s.queueMessageHint : s.messageHint)
                              : s.connectingHint,
                          hintStyle: theme.textTheme.bodyLarge?.copyWith(
                            color: theme.colorScheme.outline.withValues(
                              alpha: 0.58,
                            ),
                            fontSize: 17,
                            height: 1.38,
                          ),
                        ),
                      ),
                      const SizedBox(height: 8),
                      Row(
                        children: [
                          _CapabilityMenu(controller: _session),
                          const SizedBox(width: 5),
                          Expanded(
                            child: _ModelReasoningButton(
                              controller: _session,
                              client: widget.client,
                              onSwitchProfile: widget.onSwitchProfile,
                            ),
                          ),
                          const SizedBox(width: 5),
                          _ComposerActionButton(
                            key: const ValueKey('composer-add-button'),
                            tooltip: s.addImages,
                            icon: _pickingImages
                                ? Icons.hourglass_top_rounded
                                : Icons.add_rounded,
                            backgroundColor: Colors.transparent,
                            foregroundColor: theme.colorScheme.onSurface,
                            borderColor: theme.colorScheme.onSurface,
                            onPressed:
                                _session.phase ==
                                        SessionConnectionPhase.ready &&
                                    !_pickingImages
                                ? _pickImages
                                : null,
                          ),
                          const SizedBox(width: 5),
                          if (busy && !canSend)
                            _ComposerActionButton(
                              key: const ValueKey('composer-stop-button'),
                              tooltip: s.stop,
                              icon: Icons.stop_rounded,
                              backgroundColor: theme.colorScheme.error,
                              foregroundColor: Colors.white,
                              onPressed: _session.stop,
                            )
                          else
                            _ComposerActionButton(
                              key: const ValueKey('composer-send-button'),
                              tooltip: busy && canSend
                                  ? s.queueMessage
                                  : s.send,
                              icon: Icons.arrow_upward_rounded,
                              backgroundColor: canSend
                                  ? theme.colorScheme.primary
                                  : theme.colorScheme.surfaceContainerHighest,
                              foregroundColor: Colors.white,
                              onPressed: canSend ? _send : null,
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _AttachmentPreview extends StatelessWidget {
  const _AttachmentPreview({required this.attachment, required this.onRemove});

  final PickedImageAttachment attachment;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SizedBox.square(
      dimension: 62,
      child: Stack(
        children: [
          Positioned(
            left: 0,
            bottom: 0,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: Container(
                width: 56,
                height: 56,
                decoration: BoxDecoration(
                  color: theme.colorScheme.surfaceContainerHighest,
                  border: Border.all(color: theme.colorScheme.outlineVariant),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Image.memory(
                  attachment.bytes,
                  fit: BoxFit.cover,
                  filterQuality: FilterQuality.medium,
                  errorBuilder: (_, _, _) => Icon(
                    Icons.broken_image_outlined,
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
            ),
          ),
          Positioned(
            right: 0,
            top: 0,
            child: Tooltip(
              message: S.of(context).removeImage(attachment.name),
              child: InkWell(
                onTap: onRemove,
                customBorder: const CircleBorder(),
                child: Container(
                  width: 22,
                  height: 22,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: theme.colorScheme.inverseSurface,
                    border: Border.all(color: theme.colorScheme.surface),
                  ),
                  child: Icon(
                    Icons.close_rounded,
                    size: 15,
                    color: theme.colorScheme.onInverseSurface,
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ComposerCircleButton extends StatelessWidget {
  const _ComposerCircleButton({required this.icon});

  final IconData icon;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurface;
    return Container(
      width: 30,
      height: 30,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        border: Border.all(color: color, width: 1.5),
      ),
      alignment: Alignment.center,
      child: Icon(icon, size: 19, color: color),
    );
  }
}

class _ComposerActionButton extends StatelessWidget {
  const _ComposerActionButton({
    super.key,
    required this.tooltip,
    required this.icon,
    required this.backgroundColor,
    required this.foregroundColor,
    required this.onPressed,
    this.borderColor,
  });

  final String tooltip;
  final IconData icon;
  final Color backgroundColor;
  final Color foregroundColor;
  final Color? borderColor;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    return SizedBox.square(
      dimension: 30,
      child: IconButton(
        tooltip: tooltip,
        padding: EdgeInsets.zero,
        style: IconButton.styleFrom(
          backgroundColor: backgroundColor,
          foregroundColor: foregroundColor,
          disabledBackgroundColor: backgroundColor,
          disabledForegroundColor: Theme.of(context).colorScheme.outlineVariant,
          shape: CircleBorder(
            side: borderColor == null
                ? BorderSide.none
                : BorderSide(color: borderColor!, width: 1.5),
          ),
        ),
        icon: Icon(icon, size: 18),
        onPressed: onPressed,
      ),
    );
  }
}

class _SlashCommand {
  const _SlashCommand({
    required this.name,
    required this.description,
    this.isSkill = false,
  });

  final String name;
  final String description;
  final bool isSkill;
}

class _CapabilityMenu extends StatelessWidget {
  const _CapabilityMenu({required this.controller});

  final SessionController controller;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final label = capabilityModeLabel(s, controller.capabilityMode);
    return PopupMenuButton<String>(
      key: const ValueKey('composer-capability-button'),
      tooltip: s.capabilityModeTooltip(label),
      onSelected: controller.setCapabilityMode,
      itemBuilder: (context) => [
        for (final mode in CapabilityMode.all)
          PopupMenuItem(
            value: mode,
            child: Row(
              children: [
                if (mode == controller.capabilityMode)
                  const Icon(Icons.check, size: 16)
                else
                  const SizedBox(width: 16),
                const SizedBox(width: 8),
                Text(capabilityModeLabel(s, mode)),
              ],
            ),
          ),
      ],
      child: _ComposerCircleButton(
        icon: switch (controller.capabilityMode) {
          CapabilityMode.readOnly => Icons.lock_outline_rounded,
          CapabilityMode.fullAccess => Icons.admin_panel_settings_outlined,
          _ => Icons.edit_note_rounded,
        },
      ),
    );
  }
}

/// Model picker backed by the daemon's configured provider profiles.
///
/// Mirrors the web client: on an activated session, picking a profile sends
/// `set_model` with that profile's model; on a not-yet-activated `/new`
/// session it reconnects with the chosen profile instead.
class _ModelReasoningButton extends StatelessWidget {
  const _ModelReasoningButton({
    required this.controller,
    required this.client,
    this.onSwitchProfile,
  });

  final SessionController controller;
  final DaemonClient client;
  final ValueChanged<String>? onSwitchProfile;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final model = controller.config?.model ?? '';
    final modelLabel = model.isEmpty ? s.chooseModel : model;
    final effortLabel = reasoningEffortLabel(
      s,
      controller.config?.reasoningEffort,
    );
    final theme = Theme.of(context);
    return Tooltip(
      message: '$modelLabel · $effortLabel',
      child: InkWell(
        key: const ValueKey('composer-model-reasoning-button'),
        onTap: () => _showSettingsSheet(context),
        borderRadius: BorderRadius.circular(19),
        child: Container(
          height: 36,
          padding: const EdgeInsets.fromLTRB(10, 0, 6, 0),
          decoration: BoxDecoration(
            color: theme.colorScheme.surfaceContainerLow,
            borderRadius: BorderRadius.circular(19),
            border: Border.all(color: theme.colorScheme.outlineVariant),
          ),
          child: Row(
            children: [
              Expanded(
                child: FittedBox(
                  fit: BoxFit.scaleDown,
                  alignment: Alignment.centerLeft,
                  child: Text(
                    modelLabel,
                    maxLines: 1,
                    softWrap: false,
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: theme.colorScheme.onSurface,
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 6),
              Text(
                effortLabel,
                maxLines: 1,
                style: theme.textTheme.labelMedium?.copyWith(
                  color: theme.colorScheme.outline,
                  fontSize: 12,
                ),
              ),
              Icon(
                Icons.keyboard_arrow_down_rounded,
                size: 18,
                color: theme.colorScheme.outline,
              ),
            ],
          ),
        ),
      ),
    );
  }

  Future<void> _showSettingsSheet(BuildContext context) async {
    final destination = await showModalBottomSheet<_SettingsDestination>(
      context: context,
      showDragHandle: true,
      builder: (sheetContext) => _ModelReasoningSheet(
        model: controller.config?.model ?? '',
        reasoningEffort: controller.config?.reasoningEffort,
      ),
    );
    if (destination == null || !context.mounted) return;
    switch (destination) {
      case _SettingsDestination.model:
        await _showModelSheet(context);
        break;
      case _SettingsDestination.reasoning:
        await _showReasoningSheet(context);
        break;
    }
  }

  Future<void> _showModelSheet(BuildContext context) async {
    final s = S.of(context);
    final selected = await showModalBottomSheet<PublicProviderConfig>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (sheetContext) => _ModelSheet(
        client: client,
        currentModel: controller.config?.model ?? '',
        currentProfileId: controller.profileId ?? 'default',
      ),
    );
    if (selected == _ModelSheet.customModelMarker) {
      if (context.mounted) await _editCustomModel(context, s);
      return;
    }
    if (selected == null || !context.mounted) return;
    if (controller.sessionId != null) {
      // Activated session: only the model can be switched.
      if (selected.model != controller.config?.model) {
        controller.setModel(selected.model);
      }
    } else {
      onSwitchProfile?.call(selected.profileId);
    }
  }

  Future<void> _showReasoningSheet(BuildContext context) async {
    final selected = await showModalBottomSheet<_ReasoningSelection>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (sheetContext) =>
          _ReasoningSheet(current: controller.config?.reasoningEffort),
    );
    if (selected != null) controller.setReasoningEffort(selected.value);
  }

  Future<void> _editCustomModel(BuildContext context, S s) async {
    final text = TextEditingController(text: controller.config?.model ?? '');
    final result = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(s.setModel),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              s.customModelHint,
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const SizedBox(height: 10),
            TextField(
              controller: text,
              autofocus: true,
              decoration: InputDecoration(
                hintText: s.modelHint,
                border: const OutlineInputBorder(),
              ),
              onSubmitted: (value) => Navigator.of(context).pop(value),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: Text(s.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(text.text),
            child: Text(s.setAction),
          ),
        ],
      ),
    );
    if (result != null && result.trim().isNotEmpty) {
      controller.setModel(result);
    }
  }
}

enum _SettingsDestination { model, reasoning }

class _ModelReasoningSheet extends StatelessWidget {
  const _ModelReasoningSheet({
    required this.model,
    required this.reasoningEffort,
  });

  final String model;
  final String? reasoningEffort;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    return SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
            child: Text(
              s.modelAndReasoning,
              style: theme.textTheme.titleMedium,
            ),
          ),
          ListTile(
            leading: const Icon(Icons.language_rounded),
            title: Text(s.modelLabel),
            subtitle: Text(
              model.isEmpty ? s.chooseModel : model,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
            trailing: const Icon(Icons.chevron_right_rounded),
            onTap: () => Navigator.of(context).pop(_SettingsDestination.model),
          ),
          ListTile(
            leading: const Icon(Icons.psychology_outlined),
            title: Text(s.reasoningEffort),
            subtitle: Text(reasoningEffortLabel(s, reasoningEffort)),
            trailing: const Icon(Icons.chevron_right_rounded),
            onTap: () =>
                Navigator.of(context).pop(_SettingsDestination.reasoning),
          ),
        ],
      ),
    );
  }
}

class _ReasoningSelection {
  const _ReasoningSelection(this.value);

  final String? value;
}

class _ReasoningSheet extends StatelessWidget {
  const _ReasoningSheet({required this.current});

  final String? current;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    final choices = <_ReasoningSelection>[
      const _ReasoningSelection(null),
      for (final effort in ReasoningEffort.all) _ReasoningSelection(effort),
    ];
    return SafeArea(
      child: SizedBox(
        height: MediaQuery.sizeOf(context).height * 0.72,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
              child: Text(
                s.reasoningEffort,
                style: theme.textTheme.titleMedium,
              ),
            ),
            Expanded(
              child: ListView.builder(
                itemCount: choices.length,
                itemBuilder: (context, index) {
                  final choice = choices[index];
                  final selected = choice.value == current;
                  return ListTile(
                    leading: Icon(
                      selected
                          ? Icons.radio_button_checked
                          : Icons.radio_button_off,
                      size: 20,
                      color: selected
                          ? theme.colorScheme.primary
                          : theme.colorScheme.outline,
                    ),
                    title: Text(reasoningEffortLabel(s, choice.value)),
                    onTap: () => Navigator.of(context).pop(choice),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ModelSheet extends StatelessWidget {
  const _ModelSheet({
    required this.client,
    required this.currentModel,
    required this.currentProfileId,
  });

  final DaemonClient client;
  final String currentModel;
  final String currentProfileId;

  /// Sentinel returned when the user picks the free-text option.
  static final PublicProviderConfig customModelMarker =
      const PublicProviderConfig(profileId: '__custom__', provider: '');

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    final theme = Theme.of(context);
    return SafeArea(
      child: FutureBuilder<List<PublicProviderConfig>>(
        future: client.listProviders(),
        builder: (context, snapshot) {
          final providers = snapshot.data;
          return Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
                child: Text(s.chooseModel, style: theme.textTheme.titleMedium),
              ),
              if (providers == null)
                const Padding(
                  padding: EdgeInsets.all(32),
                  child: Center(child: CircularProgressIndicator()),
                )
              else ...[
                for (final profile in providers)
                  ListTile(
                    leading: Icon(
                      profile.profileId == currentProfileId &&
                              profile.model == currentModel
                          ? Icons.radio_button_checked
                          : Icons.radio_button_off,
                      size: 20,
                      color:
                          profile.profileId == currentProfileId &&
                              profile.model == currentModel
                          ? theme.colorScheme.primary
                          : theme.colorScheme.outline,
                    ),
                    title: Text(profile.profileId),
                    subtitle: Text(
                      '${profile.model} · ${profile.provider}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    onTap: () => Navigator.of(context).pop(profile),
                  ),
                const Divider(height: 8),
                ListTile(
                  leading: Icon(
                    Icons.edit_outlined,
                    size: 20,
                    color: theme.colorScheme.outline,
                  ),
                  title: Text(s.customModel),
                  onTap: () => Navigator.of(context).pop(customModelMarker),
                ),
              ],
            ],
          );
        },
      ),
    );
  }
}
