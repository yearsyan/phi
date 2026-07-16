import { memo, type ReactNode, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import styles from './Markdown.module.css';

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
          code: codeComponent,
          pre({ children }) {
            return <div className={styles.pre}>{children}</div>;
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
  inline?: boolean;
  className?: string;
  children?: ReactNode;
}

function codeComponent({ inline, className, children }: CodeProps) {
  const text = String(children ?? '');
  if (inline) {
    return <code className={styles.inlineCode}>{text}</code>;
  }
  const language = /language-(\w+)/.exec(className ?? '')?.[1];
  return <CodeBlock language={language} text={text} />;
}

function CodeBlock({ language, text }: { language?: string; text: string }) {
  const [copied, setCopied] = useState(false);
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
        <span className={styles.codeLang}>{language ?? 'text'}</span>
        <button type="button" className={styles.copyBtn} onClick={handleCopy}>
          {copied ? 'copied' : 'copy'}
        </button>
      </div>
      <pre className={styles.codePre}>
        <code>{text}</code>
      </pre>
    </div>
  );
}
