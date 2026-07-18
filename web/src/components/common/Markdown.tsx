import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import go from 'highlight.js/lib/languages/go';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import plaintext from 'highlight.js/lib/languages/plaintext';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import {
  Children,
  isValidElement,
  memo,
  type ReactNode,
  useMemo,
  useState,
} from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { CheckIcon, CopyIcon } from './Icons.tsx';
import styles from './Markdown.module.css';

hljs.registerLanguage('bash', bash);
hljs.registerLanguage('c', c);
hljs.registerLanguage('cpp', cpp);
hljs.registerLanguage('css', css);
hljs.registerLanguage('diff', diff);
hljs.registerLanguage('dockerfile', dockerfile);
hljs.registerLanguage('go', go);
hljs.registerLanguage('ini', ini);
hljs.registerLanguage('java', java);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('json', json);
hljs.registerLanguage('markdown', markdown);
hljs.registerLanguage('plaintext', plaintext);
hljs.registerLanguage('python', python);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('yaml', yaml);

interface MarkdownProps {
  children: string;
  /** Compact variant for inline-ish content (smaller spacing). */
  compact?: boolean;
}

/**
 * Markdown renderer with styled code blocks. Tool/assistant text is rendered as
 * GFM markdown so code fences, tables, and lists format like a coding agent UI.
 */
function MarkdownImpl({ children, compact }: MarkdownProps) {
  return (
    <div className={compact ? styles.compact : styles.root}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          code: InlineCode,
          pre: PreBlock,
          table({ children }) {
            return <table className={styles.table}>{children}</table>;
          },
          a({ children, href }) {
            return (
              <a
                className={styles.link}
                href={href}
                target="_blank"
                rel="noreferrer noopener"
              >
                {children}
              </a>
            );
          },
        }}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

export const Markdown = memo(MarkdownImpl);

interface CodeProps {
  className?: string;
  children?: ReactNode;
}

function InlineCode({ className, children }: CodeProps) {
  return (
    <code className={`${styles.inlineCode} ${className ?? ''}`.trim()}>
      {children}
    </code>
  );
}

function PreBlock({ children }: { children?: ReactNode }) {
  const child = Children.count(children) === 1 ? Children.only(children) : null;
  if (isValidElement<CodeProps>(child)) {
    const language = /language-([\w-]+)/.exec(child.props.className ?? '')?.[1];
    const text = String(child.props.children ?? '').replace(/\n$/, '');
    return <CodeBlock language={language} text={text} />;
  }
  return <pre className={styles.fallbackPre}>{children}</pre>;
}

function CodeBlock({ language, text }: { language?: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const highlighted = useMemo(
    () => highlightCode(text, language),
    [language, text],
  );
  const handleCopy = () => {
    navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      },
      () => {
        /* clipboard unavailable */
      },
    );
  };
  return (
    <div className={styles.codeBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>{highlighted.language}</span>
        <button
          type="button"
          className={styles.copyBtn}
          onClick={handleCopy}
          aria-label={copied ? 'Code copied' : 'Copy code'}
        >
          {copied ? <CheckIcon /> : <CopyIcon />}
          <span>{copied ? 'copied' : 'copy'}</span>
        </button>
      </div>
      <pre className={styles.codePre}>
        <code
          className={styles.highlightedCode}
          data-highlight-language={highlighted.language}
        >
          {highlighted.html === null ? (
            text
          ) : (
            // biome-ignore lint/security/noDangerouslySetInnerHtml: Highlight.js escapes source text before adding its token spans.
            <span dangerouslySetInnerHTML={{ __html: highlighted.html }} />
          )}
        </code>
      </pre>
    </div>
  );
}

interface HighlightedCode {
  html: string | null;
  language: string;
}

function highlightCode(text: string, language?: string): HighlightedCode {
  const normalizedLanguage = language?.trim().toLowerCase();

  try {
    if (normalizedLanguage) {
      if (!hljs.getLanguage(normalizedLanguage)) {
        return { html: null, language: normalizedLanguage };
      }
      return {
        html: hljs.highlight(text, {
          language: normalizedLanguage,
          ignoreIllegals: true,
        }).value,
        language: normalizedLanguage,
      };
    }

    const detected = hljs.highlightAuto(text);
    return {
      html: detected.value,
      language: detected.language ?? 'text',
    };
  } catch {
    return { html: null, language: normalizedLanguage ?? 'text' };
  }
}
