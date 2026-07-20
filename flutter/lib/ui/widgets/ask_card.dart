import 'package:flutter/material.dart';

import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import 'markdown_view.dart';

/// Card for an `askuser` request: 1–3 questions with single/multi-select
/// options, markdown previews, and an "Other" free-text path.
class AskCard extends StatefulWidget {
  const AskCard({super.key, required this.request, required this.onSubmit});

  final AskUserRequest request;
  final void Function(List<Json> answers) onSubmit;

  @override
  State<AskCard> createState() => _AskCardState();
}

class _AskCardState extends State<AskCard> {
  /// question index → selected option labels.
  final Map<int, Set<String>> _selected = {};

  /// question index → custom "other" text.
  final Map<int, String> _customText = {};
  final Map<int, TextEditingController> _controllers = {};
  bool _submitted = false;

  @override
  void dispose() {
    for (final controller in _controllers.values) {
      controller.dispose();
    }
    super.dispose();
  }

  bool get _canSubmit {
    for (var i = 0; i < widget.request.questions.length; i++) {
      final selected = _selected[i] ?? const {};
      final custom = _customText[i]?.trim() ?? '';
      if (selected.isEmpty && custom.isEmpty) return false;
    }
    return true;
  }

  void _submit() {
    if (!_canSubmit || _submitted) return;
    setState(() => _submitted = true);
    final answers = <Json>[
      for (var i = 0; i < widget.request.questions.length; i++)
        {
          'question_index': i,
          'selected_options': (_selected[i] ?? const <String>{}).toList(),
          'custom_text': (_customText[i]?.trim().isEmpty ?? true)
              ? null
              : _customText[i]!.trim(),
        },
    ];
    widget.onSubmit(answers);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      margin: const EdgeInsets.symmetric(vertical: 8),
      color: theme.colorScheme.secondaryContainer.withAlpha(80),
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.help_outline,
                  size: 18,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(width: 8),
                Text(
                  S.of(context).phiAsks,
                  style: theme.textTheme.labelLarge?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            for (var i = 0; i < widget.request.questions.length; i++)
              _buildQuestion(context, i, widget.request.questions[i]),
            const SizedBox(height: 8),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton(
                onPressed: _canSubmit && !_submitted ? _submit : null,
                child: Text(
                  _submitted ? S.of(context).sent : S.of(context).answer,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildQuestion(
    BuildContext context,
    int index,
    AskUserQuestion question,
  ) {
    final theme = Theme.of(context);
    final selected = _selected.putIfAbsent(index, () => <String>{});
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            question.header,
            style: theme.textTheme.labelSmall?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
          const SizedBox(height: 2),
          Text(question.question, style: theme.textTheme.bodyMedium),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final option in question.options)
                _OptionChip(
                  option: option,
                  selected: selected.contains(option.label),
                  onSelected: question.multiSelect
                      ? (value) {
                          setState(() {
                            if (value) {
                              selected.add(option.label);
                            } else {
                              selected.remove(option.label);
                            }
                          });
                        }
                      : (value) {
                          setState(() {
                            selected
                              ..clear()
                              ..add(option.label);
                          });
                        },
                ),
            ],
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _controllers.putIfAbsent(
              index,
              () => TextEditingController(),
            ),
            decoration: InputDecoration(
              isDense: true,
              hintText: S.of(context).otherHint,
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(8),
              ),
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 10,
                vertical: 8,
              ),
            ),
            onChanged: (value) => setState(() => _customText[index] = value),
          ),
        ],
      ),
    );
  }
}

class _OptionChip extends StatelessWidget {
  const _OptionChip({
    required this.option,
    required this.selected,
    required this.onSelected,
  });

  final AskUserOption option;
  final bool selected;
  final ValueChanged<bool> onSelected;

  @override
  Widget build(BuildContext context) {
    final hasPreview = (option.preview ?? '').isNotEmpty;
    return Tooltip(
      message: option.description ?? '',
      child: InkWell(
        onTap: () => onSelected(!selected),
        onLongPress: hasPreview ? () => _showPreview(context) : null,
        borderRadius: BorderRadius.circular(8),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: selected
                  ? Theme.of(context).colorScheme.primary
                  : Theme.of(context).colorScheme.outlineVariant,
              width: selected ? 1.6 : 1,
            ),
            color: selected
                ? Theme.of(context).colorScheme.primaryContainer.withAlpha(120)
                : null,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Flexible(
                child: Text(
                  option.label,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
              ),
              if (hasPreview)
                Padding(
                  padding: const EdgeInsets.only(left: 4),
                  child: Icon(
                    Icons.visibility_outlined,
                    size: 13,
                    color: Theme.of(context).colorScheme.outline,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  void _showPreview(BuildContext context) {
    showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(option.label),
        content: SingleChildScrollView(
          child: MarkdownView(text: option.preview ?? ''),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: Text(S.of(context).close),
          ),
        ],
      ),
    );
  }
}
