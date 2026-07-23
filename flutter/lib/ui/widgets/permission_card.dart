import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/models/wire.dart';
import '../../i18n/strings.dart';

/// Explicit approval UI for a tool call outside the current capability mode.
class PermissionCard extends StatefulWidget {
  const PermissionCard({
    super.key,
    required this.request,
    required this.onDecision,
  });

  final ToolPermissionPrompt request;
  final bool Function(Json decision) onDecision;

  @override
  State<PermissionCard> createState() => _PermissionCardState();
}

class _PermissionCardState extends State<PermissionCard> {
  int _suggestionIndex = 0;
  bool _submitted = false;
  Timer? _retryTimer;

  void _decide(Json decision) {
    if (_submitted || !widget.onDecision(decision)) return;
    setState(() => _submitted = true);
    _retryTimer?.cancel();
    _retryTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => _submitted = false);
    });
  }

  @override
  void dispose() {
    _retryTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    final suggestions = widget.request.suggestions;
    final selectedRule = suggestions.elementAtOrNull(_suggestionIndex);
    return Card(
      margin: const EdgeInsets.symmetric(vertical: 8),
      color: theme.colorScheme.tertiaryContainer.withAlpha(80),
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.admin_panel_settings_outlined,
                  size: 18,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(width: 8),
                Text(
                  s.permissionTitle,
                  style: theme.textTheme.labelLarge?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Text(
              s.permissionSummary(widget.request.call.name),
              style: theme.textTheme.bodyMedium,
            ),
            const SizedBox(height: 10),
            Container(
              width: double.infinity,
              constraints: const BoxConstraints(maxHeight: 220),
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: theme.colorScheme.outlineVariant),
              ),
              child: SingleChildScrollView(
                child: SelectableText(
                  _permissionTarget(widget.request.call),
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'monospace',
                  ),
                ),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              s.permissionDetails(
                toolEffectLabel(s, widget.request.effect),
                capabilityModeLabel(s, widget.request.capabilityMode),
              ),
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
            if (suggestions.length > 1) ...[
              const SizedBox(height: 10),
              DropdownButtonFormField<int>(
                initialValue: _suggestionIndex,
                decoration: InputDecoration(
                  isDense: true,
                  labelText: s.permissionRule,
                  border: const OutlineInputBorder(),
                ),
                items: [
                  for (final (index, rule) in suggestions.indexed)
                    DropdownMenuItem(value: index, child: Text(rule.label)),
                ],
                onChanged: _submitted
                    ? null
                    : (index) => setState(() => _suggestionIndex = index ?? 0),
              ),
            ] else if (selectedRule != null) ...[
              const SizedBox(height: 8),
              Text(
                '${s.permissionRule}: ${selectedRule.label}',
                style: theme.textTheme.labelSmall?.copyWith(
                  fontFamily: 'monospace',
                ),
              ),
            ],
            const SizedBox(height: 12),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              alignment: WrapAlignment.end,
              children: [
                OutlinedButton(
                  onPressed: _submitted
                      ? null
                      : () => _decide({'type': 'deny'}),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: theme.colorScheme.error,
                  ),
                  child: Text(s.permissionDeny),
                ),
                OutlinedButton(
                  onPressed: _submitted
                      ? null
                      : () => _decide({'type': 'allow_once'}),
                  child: Text(s.permissionAllowOnce),
                ),
                if (selectedRule != null)
                  FilledButton(
                    onPressed: _submitted
                        ? null
                        : () => _decide({
                            'type': 'allow_for_session',
                            'rule': selectedRule.toJson(),
                          }),
                    child: Text(s.permissionAllowSession),
                  ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

String _permissionTarget(ToolCall call) {
  final arguments = call.arguments;
  if (call.name == 'bash' && arguments is Map) {
    final command = arguments['command'];
    if (command is String) return command;
  }
  return call.argumentsPretty();
}
