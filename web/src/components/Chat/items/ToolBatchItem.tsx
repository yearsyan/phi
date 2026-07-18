import { memo, useState } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../../i18n/translations.ts';
import type { ToolTimelineItem } from '../../../state/timeline.ts';
import {
  CheckIcon,
  ChevronIcon,
  CloseIcon,
  LoaderIcon,
  WrenchIcon,
} from '../../common/Icons.tsx';
import styles from './ToolBatchItem.module.css';
import { ToolCallItem } from './ToolCallItem.tsx';

interface ToolBatchItemProps {
  tools: ToolTimelineItem[];
}

type BatchStatus = ToolTimelineItem['status'];
type ToolActivity =
  | 'read'
  | 'write'
  | 'execute'
  | 'search'
  | 'browse'
  | 'collaborate'
  | 'interact'
  | 'other';

const ACTIVITY_KEYS: Record<ToolActivity, TranslationKey> = {
  read: 'chat.toolBatch.action.read',
  write: 'chat.toolBatch.action.write',
  execute: 'chat.toolBatch.action.execute',
  search: 'chat.toolBatch.action.search',
  browse: 'chat.toolBatch.action.browse',
  collaborate: 'chat.toolBatch.action.collaborate',
  interact: 'chat.toolBatch.action.interact',
  other: 'chat.toolBatch.action.other',
};

/**
 * One assistant response can request several tools at once. Keep that
 * response to one quiet summary row until the user asks to inspect it; the
 * existing ToolCallItem remains the second-level details control.
 */
export const ToolBatchItem = memo(function ToolBatchItem({
  tools,
}: ToolBatchItemProps) {
  const { locale, t } = useI18n();
  const [open, setOpen] = useState(false);
  const status = batchStatus(tools);
  const activities = uniqueActivities(tools).map((activity) =>
    t(ACTIVITY_KEYS[activity]),
  );
  const summary = new Intl.ListFormat(locale, {
    style: 'long',
    type: 'conjunction',
  }).format(activities);

  return (
    <div className={styles.batch}>
      <button
        type="button"
        className={styles.trigger}
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
      >
        <span className={styles.batchIcon} aria-hidden="true">
          <WrenchIcon />
        </span>
        <span className={styles.summary}>{summary}</span>
        <span className={styles.count}>
          {t('chat.toolBatch.count', { count: tools.length })}
        </span>
        <span className={styles.statusSlot} aria-hidden="true">
          {status === 'running' && <LoaderIcon className={styles.spin} />}
          {status === 'done' && <CheckIcon className={styles.statusDone} />}
          {status === 'error' && <CloseIcon className={styles.statusError} />}
        </span>
        <ChevronIcon
          className={`${styles.chevron} ${open ? styles.chevronOpen : ''}`}
        />
      </button>

      {open && (
        <div className={styles.tools}>
          {tools.map((tool) => (
            <ToolCallItem
              key={tool.key}
              call={tool.call}
              status={tool.status}
              progress={tool.progress}
              output={tool.output}
              streamingArgs={tool.streamingArgs}
            />
          ))}
        </div>
      )}
    </div>
  );
});

function batchStatus(tools: ToolTimelineItem[]): BatchStatus {
  if (tools.some((tool) => tool.status === 'running')) return 'running';
  if (tools.some((tool) => tool.status === 'error')) return 'error';
  return 'done';
}

function uniqueActivities(tools: ToolTimelineItem[]): ToolActivity[] {
  const activities: ToolActivity[] = [];
  for (const tool of tools) {
    const activity = toolActivity(tool.call.name);
    if (!activities.includes(activity)) activities.push(activity);
  }
  return activities;
}

function toolActivity(name: string): ToolActivity {
  const normalized = name.toLowerCase();
  if (
    normalized.includes('bash') ||
    normalized.includes('shell') ||
    normalized.includes('exec') ||
    normalized.includes('terminal') ||
    normalized.includes('command')
  ) {
    return 'execute';
  }
  if (
    normalized.includes('write') ||
    normalized.includes('edit') ||
    normalized.includes('patch') ||
    normalized.includes('replace')
  ) {
    return 'write';
  }
  if (
    normalized.includes('grep') ||
    normalized.includes('search') ||
    normalized.includes('glob') ||
    normalized.includes('find')
  ) {
    return 'search';
  }
  if (
    normalized.includes('read') ||
    normalized.includes('list') ||
    normalized.includes('file')
  ) {
    return 'read';
  }
  if (
    normalized.includes('fetch') ||
    normalized.includes('web') ||
    normalized.includes('http') ||
    normalized.includes('browser')
  ) {
    return 'browse';
  }
  if (
    normalized.includes('agent') ||
    normalized.includes('task') ||
    normalized.includes('delegate')
  ) {
    return 'collaborate';
  }
  if (normalized.includes('ask') || normalized.includes('input')) {
    return 'interact';
  }
  return 'other';
}
