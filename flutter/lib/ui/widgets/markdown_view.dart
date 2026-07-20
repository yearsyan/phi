import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_highlight/flutter_highlight.dart';
import 'package:flutter_highlight/themes/atom-one-dark.dart';
import 'package:flutter_highlight/themes/github.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:markdown/markdown.dart' as md;

import '../../i18n/strings.dart';

/// Markdown rendering tuned for chat transcripts, with syntax-highlighted
/// fenced code blocks and a copy button.
class MarkdownView extends StatelessWidget {
  const MarkdownView({super.key, required this.text, this.dense = false});

  final String text;
  final bool dense;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final isDark = theme.brightness == Brightness.dark;
    final base = theme.textTheme.bodyMedium!;
    return MarkdownBody(
      data: text,
      selectable: true,
      softLineBreak: true,
      builders: {'code': _CodeBlockBuilder(isDark: isDark)},
      styleSheet: MarkdownStyleSheet.fromTheme(theme).copyWith(
        p: base.copyWith(fontSize: dense ? 13 : 14, height: 1.5),
        code: base.copyWith(
          fontFamily: 'Menlo',
          fontFamilyFallback: const ['monospace'],
          fontSize: 12.5,
          backgroundColor: isDark
              ? Colors.white.withAlpha(18)
              : Colors.black.withAlpha(12),
        ),
        codeblockDecoration: const BoxDecoration(),
        codeblockPadding: EdgeInsets.zero,
        blockquoteDecoration: BoxDecoration(
          border: Border(
            left: BorderSide(color: theme.colorScheme.outlineVariant, width: 3),
          ),
        ),
        listBullet: base.copyWith(fontSize: dense ? 13 : 14),
        tableBody: base.copyWith(fontSize: 12.5),
        horizontalRuleDecoration: BoxDecoration(
          border: Border(
            top: BorderSide(color: theme.colorScheme.outlineVariant),
          ),
        ),
      ),
    );
  }
}

class _CodeBlockBuilder extends MarkdownElementBuilder {
  _CodeBlockBuilder({required this.isDark});

  final bool isDark;

  @override
  Widget? visitElementAfter(md.Element element, TextStyle? preferredStyle) {
    // Inline `code` spans also reach this builder; only handle blocks (the
    // parent <pre> wraps block code in flutter_markdown's element tree).
    if (element.textContent.trim().isEmpty) return const SizedBox.shrink();
    var language = 'plaintext';
    final classAttr = element.attributes['class'];
    if (classAttr != null && classAttr.startsWith('language-')) {
      language = classAttr.substring('language-'.length);
    }
    final code = element.textContent;
    // Heuristic: single-line inline code has no trailing newline.
    final isBlock = code.contains('\n') || code.endsWith('\n');
    if (!isBlock) return null; // fall back to default inline styling
    return _CodeBlock(
      code: code.trimRight(),
      language: language,
      isDark: isDark,
    );
  }
}

class _CodeBlock extends StatelessWidget {
  const _CodeBlock({
    required this.code,
    required this.language,
    required this.isDark,
  });

  final String code;
  final String language;
  final bool isDark;

  @override
  Widget build(BuildContext context) {
    final background = isDark
        ? const Color(0xFF282C34)
        : const Color(0xFFF6F8FA);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(8),
        child: Container(
          width: double.infinity,
          color: background,
          child: Stack(
            children: [
              SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: HighlightView(
                  code,
                  language: language,
                  theme: isDark ? atomOneDarkTheme : githubTheme,
                  padding: const EdgeInsets.fromLTRB(12, 12, 44, 12),
                  textStyle: const TextStyle(
                    fontFamily: 'Menlo',
                    fontFamilyFallback: ['monospace'],
                    fontSize: 12.5,
                    height: 1.45,
                  ),
                ),
              ),
              Positioned(
                top: 2,
                right: 2,
                child: IconButton(
                  visualDensity: VisualDensity.compact,
                  iconSize: 16,
                  tooltip: S.of(context).copyCode,
                  icon: Icon(
                    Icons.copy_rounded,
                    color: isDark ? Colors.white54 : Colors.black45,
                  ),
                  onPressed: () {
                    Clipboard.setData(ClipboardData(text: code));
                  },
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
