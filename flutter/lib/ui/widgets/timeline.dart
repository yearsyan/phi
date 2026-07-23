import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../core/models/wire.dart';
import '../../i18n/strings.dart';
import '../../state/session_controller.dart';
import 'ask_card.dart';
import 'markdown_view.dart';
import 'permission_card.dart';

/// The chat transcript: committed history, run activity, live draft and
/// pending askuser and tool-permission cards, in chronological order.
class ChatTimeline extends StatelessWidget {
  const ChatTimeline({
    super.key,
    required this.controller,
    required this.scrollController,
    required this.onFork,
  });

  final SessionController controller;
  final ScrollController scrollController;

  /// Called with the transcript index of an assistant message to fork from.
  final void Function(int historyIndex) onFork;

  /// Height of the gradient that dissolves content approaching the composer.
  static const double bottomFadeHeight = 44;

  @override
  Widget build(BuildContext context) {
    final entries = _buildEntries(controller, context);
    final content = entries.isEmpty
        ? const _EmptyTranscript()
        : ListView.builder(
            controller: scrollController,
            // Bottom clearance lifts the last entry above the fade overlay.
            padding: const EdgeInsets.fromLTRB(
              12,
              12,
              12,
              bottomFadeHeight + 4,
            ),
            itemCount: entries.length,
            itemBuilder: (context, index) {
              return Align(
                alignment: Alignment.topCenter,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 860),
                  // Force full width: shrink-wrapped entries (e.g. a lone
                  // streaming "thinking" header) must stay left-aligned
                  // instead of being centred by the Align above.
                  child: SizedBox(
                    width: double.infinity,
                    child: _EntryWidget(
                      entry: entries[index],
                      onFork: onFork,
                      onAnswerAsk: controller.answerAsk,
                      onPermissionDecision: controller.decideToolPermission,
                    ),
                  ),
                ),
              );
            },
          );
    return Stack(
      fit: StackFit.expand,
      children: [
        content,
        const Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          height: bottomFadeHeight,
          child: IgnorePointer(
            child: _TimelineBottomFade(key: ValueKey('timeline-bottom-fade')),
          ),
        ),
      ],
    );
  }

  List<_Entry> _buildEntries(SessionController c, BuildContext context) {
    final entries = <_Entry>[];

    // Tool results by call id, consumed by assistant tool-call rows.
    final toolResults = <String, PublicMessage>{};
    for (final message in c.history) {
      if (message.role == 'tool' && message.toolCallId != null) {
        toolResults[message.toolCallId!] = message;
      }
    }

    final compactionAt = <int, List<CompactionMarker>>{};
    for (final marker in c.compactions) {
      compactionAt.putIfAbsent(marker.historyIndex, () => []).add(marker);
    }

    for (var i = 0; i < c.history.length; i++) {
      for (final marker in compactionAt[i] ?? const <CompactionMarker>[]) {
        entries.add(_CompactionEntry(marker));
      }
      final message = c.history[i];
      if (!message.isPublic) continue;
      switch (message.role) {
        case 'user':
          entries.add(_UserEntry(message: message));
        case 'assistant':
          entries.add(
            _AssistantEntry(
              message: message,
              historyIndex: i,
              toolResults: {
                for (final call in message.toolCalls)
                  if (toolResults.containsKey(call.id))
                    call.id: toolResults[call.id]!,
              },
            ),
          );
        default:
          break; // tool results are rendered inside their assistant call
      }
    }
    for (final marker in c.compactions) {
      if (marker.historyIndex >= c.history.length) {
        entries.add(_CompactionEntry(marker));
      }
    }

    // Optimistic (not yet echoed) prompts.
    for (final prompt in c.pendingPrompts) {
      entries.add(_PendingUserEntry(prompt));
    }

    // Run activity. Tool calls already committed to history at/after the
    // run's start render inline with their assistant message (mirrors the
    // web client's deriveTimeline); only steps history does not cover yet
    // are appended here, so finished work never piles up after the final
    // answer.
    final run = c.activeRun;
    if (run != null) {
      final represented = <String>{
        for (var i = run.historyStart; i < c.history.length; i++)
          if (c.history[i].role == 'assistant')
            for (final call in c.history[i].toolCalls)
              '${call.name}:${call.id}',
      };
      final draftKeys = <String>{
        for (final call in c.draft?.toolCalls ?? const <ToolCallDraft>[])
          if (call.id != null && call.name != null) '${call.name}:${call.id}',
      };
      for (final turn in run.turns) {
        for (final step in turn.steps) {
          switch (step) {
            case ToolStep():
              final key = '${step.call.name}:${step.call.id}';
              if (!represented.contains(key) && !draftKeys.contains(key)) {
                entries.add(_LiveToolEntry(step));
              }
            case RetryStep():
              entries.add(
                _StatusEntry(
                  S
                      .of(context)
                      .providerRetry(
                        step.retryNumber,
                        step.maxRetries,
                        step.reason,
                      ),
                  level: 'warn',
                ),
              );
            case SubagentStep():
              entries.add(
                _StatusEntry(step.message, level: 'info', detail: step.detail),
              );
            case NoticeStep():
              entries.add(_StatusEntry(step.message, level: step.level));
          }
        }
      }
      if (run.status != 'running') {
        final s = S.of(context);
        entries.add(
          _StatusEntry(switch (run.status) {
            'completed' => s.runCompleted,
            'stopped' => s.runStopped,
            'failed' => s.runFailed(run.errorMessage),
            _ => run.status,
          }, level: run.status == 'failed' ? 'error' : 'info'),
        );
      }
    }

    // Live draft / busy indicator.
    final draft = c.draft;
    final draftEmpty =
        draft == null ||
        (draft.text.isEmpty &&
            draft.reasoning.isEmpty &&
            draft.toolCalls.isEmpty);
    if (!draftEmpty) {
      entries.add(_DraftEntry(draft));
    } else if (c.activeRunId != null && c.status == SessionStatus.running) {
      entries.add(const _ThinkingEntry());
    }

    // Pending askuser cards.
    for (final ask in c.pendingAsks) {
      entries.add(_AskEntry(ask));
    }
    for (final request in c.pendingToolPermissions) {
      entries.add(_PermissionEntry(request));
    }

    return entries;
  }
}

/* ------------------------------------------------------------------------- */
/* Entries                                                                   */
/* ------------------------------------------------------------------------- */

sealed class _Entry {
  const _Entry();
}

class _UserEntry extends _Entry {
  const _UserEntry({required this.message});
  final PublicMessage message;
}

class _PendingUserEntry extends _Entry {
  const _PendingUserEntry(this.prompt);
  final PendingPrompt prompt;
}

class _AssistantEntry extends _Entry {
  const _AssistantEntry({
    required this.message,
    required this.historyIndex,
    required this.toolResults,
  });
  final PublicMessage message;
  final int historyIndex;
  final Map<String, PublicMessage> toolResults;
}

class _DraftEntry extends _Entry {
  const _DraftEntry(this.draft);
  final AssistantDraft draft;
}

class _LiveToolEntry extends _Entry {
  const _LiveToolEntry(this.step);
  final ToolStep step;
}

class _StatusEntry extends _Entry {
  const _StatusEntry(this.text, {this.level = 'info', this.detail});
  final String text;
  final String level;
  final String? detail;
}

class _CompactionEntry extends _Entry {
  const _CompactionEntry(this.marker);
  final CompactionMarker marker;
}

class _ThinkingEntry extends _Entry {
  const _ThinkingEntry();
}

class _AskEntry extends _Entry {
  const _AskEntry(this.request);
  final AskUserRequest request;
}

class _PermissionEntry extends _Entry {
  const _PermissionEntry(this.request);
  final ToolPermissionPrompt request;
}

/* ------------------------------------------------------------------------- */
/* Entry rendering                                                           */
/* ------------------------------------------------------------------------- */

class _EntryWidget extends StatelessWidget {
  const _EntryWidget({
    required this.entry,
    required this.onFork,
    required this.onAnswerAsk,
    required this.onPermissionDecision,
  });

  final _Entry entry;
  final void Function(int historyIndex) onFork;
  final bool Function(String askId, List<Json> answers) onAnswerAsk;
  final bool Function(String permissionId, Json decision) onPermissionDecision;

  @override
  Widget build(BuildContext context) {
    final s = S.of(context);
    return switch (entry) {
      _UserEntry(message: final message) => _UserBubble(message: message),
      _PendingUserEntry(prompt: final prompt) => _UserBubble(
        content: prompt.content,
        status: prompt.status == 'queued'
            ? s.queuedAt(prompt.queuePosition)
            : s.sending,
      ),
      _AssistantEntry(
        message: final message,
        historyIndex: final historyIndex,
        toolResults: final toolResults,
      ) =>
        _AssistantMessage(
          message: message,
          toolResults: toolResults,
          onFork: () => onFork(historyIndex),
        ),
      _DraftEntry(draft: final draft) => _LiveDraft(draft: draft),
      _LiveToolEntry(step: final step) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 3),
        child: ToolCallTile(
          call: step.call,
          running: !step.done,
          isError: step.isError,
          resultContent: step.content,
          progress: step.progress,
        ),
      ),
      _StatusEntry(
        text: final text,
        level: final level,
        detail: final detail,
      ) =>
        _StatusLine(text: text, level: level, detail: detail),
      _CompactionEntry(marker: final marker) => _CompactionDivider(
        marker: marker,
      ),
      _ThinkingEntry() => const _ThinkingRow(),
      _AskEntry(request: final request) => AskCard(
        request: request,
        onSubmit: (answers) => onAnswerAsk(request.askId, answers),
      ),
      _PermissionEntry(request: final request) => PermissionCard(
        key: ValueKey(request.permissionId),
        request: request,
        onDecision: (decision) =>
            onPermissionDecision(request.permissionId, decision),
      ),
    };
  }
}

class _EmptyTranscript extends StatelessWidget {
  const _EmptyTranscript();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.chat_bubble_outline,
            size: 44,
            color: theme.colorScheme.outline,
          ),
          const SizedBox(height: 12),
          Text(
            S.of(context).sendMessageToStart,
            style: theme.textTheme.bodyMedium?.copyWith(
              color: theme.colorScheme.outline,
            ),
          ),
        ],
      ),
    );
  }
}

/// Soft gradient that dissolves transcript content as it scrolls towards the
/// composer, replacing the hard clip at the list's bottom edge.
class _TimelineBottomFade extends StatelessWidget {
  const _TimelineBottomFade({super.key});

  @override
  Widget build(BuildContext context) {
    final background = Theme.of(context).scaffoldBackgroundColor;
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [background.withValues(alpha: 0), background],
        ),
      ),
    );
  }
}

/* ------------------------------- user bubble ----------------------------- */

class _UserBubble extends StatelessWidget {
  const _UserBubble({this.message, this.content, this.status});

  final PublicMessage? message;
  final Content? content;
  final String? status;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final resolvedContent = content ?? message?.content;
    final contentText = resolvedContent?.plainText ?? '';
    final images = resolvedContent?.isParts ?? false
        ? resolvedContent!.parts
              .where((part) => part.type == 'image_url')
              .toList()
        : const <ContentPart>[];
    if (contentText.isEmpty && images.isEmpty) return const SizedBox.shrink();
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        margin: const EdgeInsets.only(left: 48, top: 4, bottom: 4),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          color: theme.colorScheme.primaryContainer.withAlpha(150),
          borderRadius: BorderRadius.circular(14),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            if (images.isNotEmpty)
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final image in images)
                    ClipRRect(
                      borderRadius: BorderRadius.circular(10),
                      child: SizedBox(
                        width: images.length == 1 ? 220 : 104,
                        height: images.length == 1 ? 180 : 104,
                        child: _ContentImage(part: image),
                      ),
                    ),
                ],
              ),
            if (images.isNotEmpty && contentText.isNotEmpty)
              const SizedBox(height: 8),
            if (contentText.isNotEmpty)
              SelectableText(contentText, style: theme.textTheme.bodyMedium),
            if (status != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  status!,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: theme.colorScheme.outline,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _ContentImage extends StatelessWidget {
  const _ContentImage({required this.part});

  final ContentPart part;

  @override
  Widget build(BuildContext context) {
    final url = part.imageUrl;
    final fallback = ColoredBox(
      color: Theme.of(context).colorScheme.surfaceContainerHighest,
      child: Icon(
        Icons.broken_image_outlined,
        color: Theme.of(context).colorScheme.outline,
      ),
    );
    if (url == null || url.isEmpty) return fallback;
    final uri = Uri.tryParse(url);
    final data = uri?.data;
    if (data != null) {
      try {
        return Image.memory(
          data.contentAsBytes(),
          fit: BoxFit.cover,
          filterQuality: FilterQuality.medium,
          errorBuilder: (_, _, _) => fallback,
        );
      } catch (_) {
        return fallback;
      }
    }
    if (uri != null && (uri.scheme == 'http' || uri.scheme == 'https')) {
      return Image.network(
        url,
        fit: BoxFit.cover,
        filterQuality: FilterQuality.medium,
        errorBuilder: (_, _, _) => fallback,
      );
    }
    return fallback;
  }
}

/* ---------------------------- assistant message -------------------------- */

class _AssistantMessage extends StatelessWidget {
  const _AssistantMessage({
    required this.message,
    required this.toolResults,
    required this.onFork,
  });

  final PublicMessage message;
  final Map<String, PublicMessage> toolResults;
  final VoidCallback onFork;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final hasReasoning = (message.reasoning ?? '').isNotEmpty;
    final hasText = (message.content?.plainText ?? '').isNotEmpty;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (hasReasoning) ReasoningBlock(reasoning: message.reasoning!),
          if (hasText) MarkdownView(text: message.content!.plainText),
          for (final call in message.toolCalls)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: ToolCallTile(
                call: call,
                running: false,
                isError: toolResults[call.id]?.toolResultIsError ?? false,
                resultContent: toolResults[call.id]?.content?.plainText,
              ),
            ),
          if (hasText)
            Row(
              children: [
                _ActionIcon(
                  icon: Icons.copy_outlined,
                  tooltip: S.of(context).copy,
                  onPressed: () => Clipboard.setData(
                    ClipboardData(text: message.content!.plainText),
                  ),
                ),
                _ActionIcon(
                  icon: Icons.call_split_rounded,
                  tooltip: S.of(context).forkFromReply,
                  onPressed: onFork,
                ),
                const Spacer(),
              ],
            )
          else if (!hasReasoning && message.toolCalls.isEmpty)
            Text(
              S.of(context).emptyMessage,
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.outline,
              ),
            ),
        ],
      ),
    );
  }
}

class _ActionIcon extends StatelessWidget {
  const _ActionIcon({
    required this.icon,
    required this.tooltip,
    required this.onPressed,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    return IconButton(
      onPressed: onPressed,
      tooltip: tooltip,
      icon: Icon(icon, size: 15),
      visualDensity: VisualDensity.compact,
      color: Theme.of(context).colorScheme.outline,
    );
  }
}

/* ------------------------------ live draft ------------------------------- */

class _LiveDraft extends StatelessWidget {
  const _LiveDraft({required this.draft});

  final AssistantDraft draft;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (draft.reasoning.isNotEmpty)
            ReasoningBlock(reasoning: draft.reasoning, streaming: true),
          if (draft.text.isNotEmpty) MarkdownView(text: draft.text),
          for (final toolCall in draft.toolCalls)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: ToolCallTile(
                call: ToolCall(
                  id: toolCall.id ?? '',
                  name: toolCall.name ?? 'tool',
                  arguments: toolCall.arguments,
                ),
                running: true,
                isError: false,
                streamingArguments: toolCall.arguments,
              ),
            ),
        ],
      ),
    );
  }
}

/* ----------------------------- reasoning block --------------------------- */

class ReasoningBlock extends StatefulWidget {
  const ReasoningBlock({
    super.key,
    required this.reasoning,
    this.streaming = false,
  });

  final String reasoning;
  final bool streaming;

  @override
  State<ReasoningBlock> createState() => _ReasoningBlockState();
}

class _ReasoningBlockState extends State<ReasoningBlock> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            onTap: () => setState(() => _expanded = !_expanded),
            borderRadius: BorderRadius.circular(6),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    _expanded ? Icons.expand_less : Icons.expand_more,
                    size: 16,
                    color: theme.colorScheme.outline,
                  ),
                  const SizedBox(width: 4),
                  Text(
                    widget.streaming
                        ? S.of(context).thinkingStreaming
                        : S.of(context).thinking,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                      fontStyle: FontStyle.italic,
                    ),
                  ),
                ],
              ),
            ),
          ),
          if (_expanded)
            Container(
              width: double.infinity,
              margin: const EdgeInsets.only(top: 2, bottom: 4),
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest.withAlpha(120),
                borderRadius: BorderRadius.circular(8),
                border: Border(
                  left: BorderSide(
                    color: theme.colorScheme.outlineVariant,
                    width: 3,
                  ),
                ),
              ),
              child: SelectableText(
                widget.reasoning,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                  fontStyle: FontStyle.italic,
                  height: 1.45,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/* ------------------------------ tool call tile --------------------------- */

class ToolCallTile extends StatefulWidget {
  const ToolCallTile({
    super.key,
    required this.call,
    required this.running,
    required this.isError,
    this.resultContent,
    this.progress = const [],
    this.streamingArguments,
  });

  final ToolCall call;
  final bool running;
  final bool isError;
  final String? resultContent;
  final List<String> progress;
  final String? streamingArguments;

  @override
  State<ToolCallTile> createState() => _ToolCallTileState();
}

class _ToolCallTileState extends State<ToolCallTile> {
  bool _expanded = false;

  IconData _iconFor(String name) {
    final lower = name.toLowerCase();
    if (lower.contains('bash') || lower.contains('shell')) {
      return Icons.terminal_rounded;
    }
    if (lower.contains('read')) return Icons.description_outlined;
    if (lower.contains('edit') || lower.contains('write')) {
      return Icons.edit_outlined;
    }
    if (lower.contains('grep') || lower.contains('search')) {
      return Icons.search_rounded;
    }
    if (lower.contains('glob') || lower.contains('list')) {
      return Icons.folder_outlined;
    }
    if (lower.contains('web')) return Icons.language_rounded;
    if (lower.contains('task') || lower.contains('agent')) {
      return Icons.account_tree_outlined;
    }
    return Icons.build_outlined;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final accent = widget.isError
        ? theme.colorScheme.error
        : widget.running
        ? theme.colorScheme.primary
        : theme.colorScheme.outline;
    return Material(
      color: theme.colorScheme.surfaceContainerHigh.withAlpha(110),
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        onTap: () => setState(() => _expanded = !_expanded),
        borderRadius: BorderRadius.circular(8),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Icon(_iconFor(widget.call.name), size: 15, color: accent),
                  const SizedBox(width: 8),
                  Text(
                    widget.call.name,
                    style: theme.textTheme.labelMedium?.copyWith(
                      fontFamily: 'Menlo',
                      fontFamilyFallback: const ['monospace'],
                      color: accent,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      widget.call.summary(),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: theme.colorScheme.outline,
                      ),
                    ),
                  ),
                  if (widget.running)
                    const SizedBox(
                      width: 12,
                      height: 12,
                      child: CircularProgressIndicator(strokeWidth: 1.6),
                    )
                  else if (widget.isError)
                    Icon(
                      Icons.error_outline,
                      size: 14,
                      color: theme.colorScheme.error,
                    )
                  else
                    Icon(
                      Icons.check_circle_outline,
                      size: 14,
                      color: theme.colorScheme.outline,
                    ),
                  const SizedBox(width: 4),
                  Icon(
                    _expanded ? Icons.expand_less : Icons.expand_more,
                    size: 15,
                    color: theme.colorScheme.outline,
                  ),
                ],
              ),
              if (widget.progress.isNotEmpty && !_expanded)
                Padding(
                  padding: const EdgeInsets.only(left: 23, top: 2),
                  child: Text(
                    widget.progress.last,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: theme.colorScheme.outline,
                    ),
                  ),
                ),
              if (_expanded) ...[
                const SizedBox(height: 8),
                _DetailSection(
                  title: S.of(context).arguments,
                  content:
                      widget.streamingArguments ??
                      widget.call.argumentsPretty(),
                  isCode: true,
                ),
                if (widget.progress.isNotEmpty)
                  _DetailSection(
                    title: S.of(context).progress,
                    content: widget.progress.join('\n'),
                  ),
                if (widget.resultContent != null &&
                    widget.resultContent!.isNotEmpty)
                  _DetailSection(
                    title: widget.isError
                        ? S.of(context).errorLabel
                        : S.of(context).result,
                    content: widget.resultContent!,
                    isError: widget.isError,
                  ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _DetailSection extends StatelessWidget {
  const _DetailSection({
    required this.title,
    required this.content,
    this.isCode = false,
    this.isError = false,
  });

  final String title;
  final String content;
  final bool isCode;
  final bool isError;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(
                title,
                style: theme.textTheme.labelSmall?.copyWith(
                  color: isError
                      ? theme.colorScheme.error
                      : theme.colorScheme.outline,
                ),
              ),
              const SizedBox(width: 6),
              GestureDetector(
                onTap: () => Clipboard.setData(ClipboardData(text: content)),
                child: Icon(
                  Icons.copy_outlined,
                  size: 12,
                  color: theme.colorScheme.outline,
                ),
              ),
            ],
          ),
          const SizedBox(height: 3),
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 260),
            child: SingleChildScrollView(
              child: Container(
                width: double.infinity,
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: theme.colorScheme.surfaceContainerHighest.withAlpha(
                    isError ? 60 : 120,
                  ),
                  borderRadius: BorderRadius.circular(6),
                ),
                child: SelectableText(
                  content.length > 12000
                      ? '${content.substring(0, 12000)}\n${S.of(context).truncated}'
                      : content,
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: isCode ? 'Menlo' : null,
                    fontFamilyFallback: const ['monospace'],
                    fontSize: 11.5,
                    height: 1.4,
                    color: isError ? theme.colorScheme.error : null,
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

/* -------------------------------- status line ---------------------------- */

class _StatusLine extends StatelessWidget {
  const _StatusLine({required this.text, this.level = 'info', this.detail});

  final String text;
  final String level;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = switch (level) {
      'error' => theme.colorScheme.error,
      'warn' => Colors.orange,
      _ => theme.colorScheme.outline,
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.subdirectory_arrow_right_rounded, size: 13, color: color),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              detail != null ? '$text  ($detail)' : text,
              style: theme.textTheme.labelSmall?.copyWith(
                color: color,
                fontStyle: FontStyle.italic,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/* ---------------------------- compaction divider ------------------------- */

class _CompactionDivider extends StatelessWidget {
  const _CompactionDivider({required this.marker});

  final CompactionMarker marker;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final s = S.of(context);
    final (label, color) = switch (marker.phase) {
      'started' => (s.compactingContext, theme.colorScheme.outline),
      'completed' => (s.contextCompacted, theme.colorScheme.outline),
      _ => (s.compactionFailed(marker.message), theme.colorScheme.error),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Row(
        children: [
          Expanded(child: Divider(color: color.withAlpha(100))),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10),
            child: Row(
              children: [
                if (marker.phase == 'started')
                  const SizedBox(
                    width: 11,
                    height: 11,
                    child: CircularProgressIndicator(strokeWidth: 1.4),
                  )
                else
                  Icon(Icons.compress_outlined, size: 13, color: color),
                const SizedBox(width: 6),
                Text(
                  label,
                  style: theme.textTheme.labelSmall?.copyWith(color: color),
                ),
              ],
            ),
          ),
          Expanded(child: Divider(color: color.withAlpha(100))),
        ],
      ),
    );
  }
}

/* ------------------------------- thinking row ---------------------------- */

class _ThinkingRow extends StatelessWidget {
  const _ThinkingRow();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          SizedBox(
            width: 14,
            height: 14,
            child: CircularProgressIndicator(
              strokeWidth: 1.6,
              color: theme.colorScheme.primary,
            ),
          ),
          const SizedBox(width: 10),
          Text(
            S.of(context).working,
            style: theme.textTheme.labelMedium?.copyWith(
              color: theme.colorScheme.outline,
              fontStyle: FontStyle.italic,
            ),
          ),
        ],
      ),
    );
  }
}
