import { type CSSProperties, memo } from 'react';
import type { ListRange } from 'react-virtuoso';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TimelineItem, ToolTimelineItem } from '../../state/timeline.ts';
import type { Content, ToolCall } from '../../types/wire.ts';
import styles from './ConversationMap.module.css';

interface ConversationMapProps {
  items: TimelineItem[];
  visibleRange: ListRange | null;
  onJump: (index: number) => void;
}

export interface ConversationMapEntry {
  key: string;
  startIndex: number;
  endIndex: number;
  kind: 'turn' | 'assistant';
  title: string;
  excerpt: string;
  resources: string[];
}

/**
 * Compact turn-level outline for long conversations. Tool rows stay attached
 * to their owning user turn so tool-heavy responses do not flood the rail.
 */
export const ConversationMap = memo(function ConversationMap({
  items,
  visibleRange,
  onJump,
}: ConversationMapProps) {
  const { t } = useI18n();
  const entries = buildConversationMapEntries(items);
  if (entries.length < 2) return null;

  const activeKey = activeConversationMapEntryKey(entries, visibleRange);
  const rootStyle = {
    '--map-track-height': `${Math.min(entries.length * 16, 320)}px`,
  } as CSSProperties;

  return (
    <nav
      className={styles.root}
      style={rootStyle}
      aria-label={t('chat.map.label')}
    >
      <div className={styles.track}>
        {entries.map((entry, index) => {
          const title =
            entry.title ||
            (entry.kind === 'turn'
              ? t('chat.map.userFallback')
              : t('chat.map.assistantFallback'));
          const position = ((index + 0.5) / entries.length) * 100;
          const previewAlignment =
            position < 22
              ? styles.previewTop
              : position > 78
                ? styles.previewBottom
                : styles.previewCenter;
          const style = {
            '--map-position': `${position}%`,
          } as CSSProperties;
          const active = entry.key === activeKey;

          return (
            <button
              type="button"
              key={entry.key}
              className={`${styles.marker} ${active ? styles.markerActive : ''}`}
              style={style}
              onClick={() => onJump(entry.startIndex)}
              aria-label={t('chat.map.jump', { title })}
              aria-current={active ? 'location' : undefined}
            >
              <span className={styles.markerLine} aria-hidden="true" />
              <span
                className={`${styles.preview} ${previewAlignment}`}
                aria-hidden="true"
              >
                <strong className={styles.previewTitle}>{title}</strong>
                {entry.excerpt && (
                  <span className={styles.previewExcerpt}>{entry.excerpt}</span>
                )}
                {entry.resources.length > 0 && (
                  <span className={styles.resources}>
                    {entry.resources.map((resource) => (
                      <span className={styles.resource} key={resource}>
                        <span className={styles.resourceIcon}>#</span>
                        {resource}
                      </span>
                    ))}
                  </span>
                )}
              </span>
            </button>
          );
        })}
      </div>
    </nav>
  );
});

export function buildConversationMapEntries(
  items: TimelineItem[],
): ConversationMapEntry[] {
  const entries: ConversationMapEntry[] = [];
  let current: ConversationMapEntry | null = null;

  const startEntry = (
    item: TimelineItem,
    index: number,
    kind: ConversationMapEntry['kind'],
    title: string,
  ) => {
    if (current !== null) current.endIndex = Math.max(index - 1, 0);
    const entry: ConversationMapEntry = {
      key: `map-${item.key}`,
      startIndex: index,
      endIndex: index,
      kind,
      title,
      excerpt: '',
      resources: [],
    };
    current = entry;
    entries.push(entry);
  };

  items.forEach((item, index) => {
    if (item.kind === 'user') {
      startEntry(
        item,
        index,
        'turn',
        previewContent(item.message.content, 120),
      );
      return;
    }

    if (item.kind === 'assistant') {
      if (current === null) startEntry(item, index, 'assistant', '');
      if (current !== null) {
        const response = previewText(item.text || item.reasoning, 360);
        current.excerpt = joinPreview(current.excerpt, response, 360);
        current.endIndex = index;
      }
      return;
    }

    if (current === null) return;
    current.endIndex = index;

    if (item.kind === 'tool') {
      appendResource(current.resources, toolResource(item));
    } else if (item.kind === 'tool-batch') {
      for (const tool of item.tools) {
        appendResource(current.resources, toolResource(tool));
      }
    } else if (item.kind === 'status' && !current.excerpt) {
      current.excerpt = previewText(statusPreview(item), 360);
    } else if (item.kind === 'compaction' && !current.excerpt) {
      current.excerpt = previewText(item.message ?? '', 360);
    }
  });

  const lastEntry = entries.at(-1);
  if (lastEntry !== undefined) {
    lastEntry.endIndex = Math.max(items.length - 1, 0);
  }
  return entries;
}

export function activeConversationMapEntryKey(
  entries: ConversationMapEntry[],
  visibleRange: ListRange | null,
): string | null {
  if (entries.length === 0) return null;
  if (visibleRange === null) return entries.at(-1)?.key ?? null;

  const anchor = (visibleRange.startIndex + visibleRange.endIndex) / 2;
  let active = entries[0];
  for (const entry of entries) {
    if (entry.startIndex > anchor) break;
    active = entry;
  }
  return active?.key ?? null;
}

function previewContent(content: Content | null, maxLength: number): string {
  if (content === null) return '';
  if (content.type === 'text') return previewText(content.value, maxLength);
  return previewText(
    content.value
      .filter((part) => part.type === 'text')
      .map((part) => part.text)
      .join(' '),
    maxLength,
  );
}

function previewText(value: string, maxLength: number): string {
  const normalized = value
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/```[\w-]*\s*/g, ' ')
    .replace(/[`*_~>#|]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (normalized.length <= maxLength) return normalized;
  return `${normalized.slice(0, maxLength - 1).trimEnd()}…`;
}

function joinPreview(current: string, next: string, maxLength: number): string {
  if (!next) return current;
  return previewText(current ? `${current} ${next}` : next, maxLength);
}

function toolResource(item: ToolTimelineItem): string {
  const path = toolArgument(item.call, ['path', 'file_path']);
  if (path) return pathLabel(path);
  return item.call.name || 'tool';
}

function toolArgument(call: ToolCall, keys: string[]): string | null {
  if (
    call.arguments === null ||
    typeof call.arguments !== 'object' ||
    Array.isArray(call.arguments)
  ) {
    return null;
  }
  const args = call.arguments as Record<string, unknown>;
  for (const key of keys) {
    const value = args[key];
    if (typeof value === 'string' && value.trim()) return value.trim();
  }
  return null;
}

function pathLabel(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  return normalized.split(/[\\/]/).pop() || normalized || path;
}

function appendResource(resources: string[], resource: string): void {
  if (!resource || resources.includes(resource) || resources.length >= 4)
    return;
  resources.push(resource);
}

function statusPreview(
  item: Extract<TimelineItem, { kind: 'status' }>,
): string {
  switch (item.step.kind) {
    case 'notice':
      return item.step.message;
    case 'subagent':
      return item.step.detail ?? item.step.message;
    case 'retry':
      return item.step.reason;
  }
}
