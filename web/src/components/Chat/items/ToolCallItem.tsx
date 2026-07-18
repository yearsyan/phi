import { memo, useState } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { ToolCall } from '../../../types/wire.ts';
import {
  AgentIcon,
  CheckIcon,
  ChevronIcon,
  CloseIcon,
  EditIcon,
  FileIcon,
  GlobeIcon,
  ListIcon,
  LoaderIcon,
  SearchIcon,
  TerminalIcon,
  WrenchIcon,
} from '../../common/Icons.tsx';
import styles from './ToolCallItem.module.css';

interface ToolCallItemProps {
  call: ToolCall;
  status: 'running' | 'done' | 'error';
  progress: string[];
  output: string | null;
  streamingArgs: string | null;
}

/**
 * Terminal-style tool row: one collapsible line with icon, tool name, primary
 * parameter and a status icon. The same component renders live steps and
 * committed history, so resyncs no longer switch visuals.
 */
export const ToolCallItem = memo(function ToolCallItem({
  call,
  status,
  progress,
  output,
  streamingArgs,
}: ToolCallItemProps) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);

  const expandable =
    streamingArgs !== null || call.arguments != null || output !== null;
  const summary = primaryParam(call);
  const lastProgress =
    progress.length > 0 ? (progress[progress.length - 1] ?? null) : null;

  return (
    <div className={styles.row}>
      <button
        type="button"
        className={`${styles.trigger} ${expandable ? '' : styles.triggerStatic}`}
        onClick={() => expandable && setOpen((value) => !value)}
        aria-expanded={expandable ? open : undefined}
      >
        <span className={styles.toolIcon} aria-hidden="true">
          {toolIcon(call.name)}
        </span>
        <span className={styles.toolName}>
          {call.name || t('chat.toolResult')}
        </span>
        {!open && status === 'running' && lastProgress !== null ? (
          <span className={styles.paramSummary}>{lastProgress}</span>
        ) : (
          !open &&
          summary !== null && (
            <span className={styles.paramSummary}>({summary})</span>
          )
        )}
        <span className={styles.statusSlot} aria-hidden="true">
          {status === 'running' && <LoaderIcon className={styles.spin} />}
          {status === 'done' && <CheckIcon className={styles.statusDone} />}
          {status === 'error' && <CloseIcon className={styles.statusError} />}
        </span>
        {expandable && (
          <ChevronIcon
            className={`${styles.chevron} ${open ? styles.chevronOpen : ''}`}
          />
        )}
      </button>

      {open && expandable && (
        <div className={styles.details}>
          {streamingArgs !== null ? (
            <pre className={styles.code}>{streamingArgs}…</pre>
          ) : (
            <ArgsList args={call.arguments} />
          )}
          {output !== null && (
            <>
              <div
                className={`${styles.sectionLabel} ${status === 'error' ? styles.sectionLabelError : ''}`}
              >
                {status === 'error'
                  ? t('chat.activity.toolFailed')
                  : t('chat.activity.output')}
              </div>
              <pre
                className={`${styles.code} ${status === 'error' ? styles.codeError : ''}`}
              >
                {output}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
});

function ArgsList({ args }: { args: unknown }) {
  if (args == null) return null;
  if (typeof args !== 'object' || Array.isArray(args)) {
    return <pre className={styles.code}>{safeStringify(args)}</pre>;
  }
  const entries = Object.entries(args as Record<string, unknown>);
  if (entries.length === 0) return null;
  return (
    <div className={styles.args}>
      {entries.map(([key, value]) => (
        <ArgRow key={key} name={key} value={value} />
      ))}
    </div>
  );
}

function ArgRow({ name, value }: { name: string; value: unknown }) {
  const text = typeof value === 'string' ? value : safeStringify(value);
  if (text.length > 120 || text.includes('\n')) {
    return (
      <div className={styles.argBlock}>
        <span className={styles.argKey}>{name}</span>
        <pre className={styles.code}>{text}</pre>
      </div>
    );
  }
  return (
    <div className={styles.argInline}>
      <span className={styles.argKey}>{name}</span>
      <span className={styles.argValue} title={text}>
        {text}
      </span>
    </div>
  );
}

const PRIMARY_PARAM_KEYS = [
  'path',
  'file_path',
  'command',
  'cmd',
  'url',
  'pattern',
  'query',
  'prompt',
  'skill',
] as const;

function primaryParam(call: ToolCall): string | null {
  const args = call.arguments;
  if (args === null || typeof args !== 'object' || Array.isArray(args)) {
    return null;
  }
  const record = args as Record<string, unknown>;
  for (const key of PRIMARY_PARAM_KEYS) {
    const value = record[key];
    if (typeof value === 'string' && value.length > 0) {
      const single = value.replace(/\s+/g, ' ').trim();
      return single.length > 50 ? `${single.slice(0, 50)}…` : single;
    }
  }
  return null;
}

function toolIcon(name: string) {
  const n = name.toLowerCase();
  if (n.includes('shell') || n.includes('bash') || n.includes('exec')) {
    return <TerminalIcon />;
  }
  if (n.includes('grep') || n.includes('search') || n.includes('glob')) {
    return <SearchIcon />;
  }
  if (n.includes('read') || n.includes('file')) {
    return <FileIcon />;
  }
  if (n.includes('write') || n.includes('edit') || n.includes('patch')) {
    return <EditIcon />;
  }
  if (n.includes('fetch') || n.includes('web') || n.includes('http')) {
    return <GlobeIcon />;
  }
  if (n.includes('plan') || n.includes('todo')) {
    return <ListIcon />;
  }
  if (n.includes('agent') || n.includes('task')) {
    return <AgentIcon />;
  }
  return <WrenchIcon />;
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}
