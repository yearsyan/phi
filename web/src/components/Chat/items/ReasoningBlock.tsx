import { memo } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import { ChevronIcon, SparkIcon } from '../../common/Icons.tsx';
import { Markdown } from '../../common/Markdown.tsx';
import styles from './ReasoningBlock.module.css';

interface ReasoningBlockProps {
  reasoning: string;
  streaming: boolean;
}

/**
 * Provider-normalized reasoning rendered in a compact Kimi-style disclosure.
 * It is intentionally uncontrolled and closed by default.
 */
export const ReasoningBlock = memo(function ReasoningBlock({
  reasoning,
  streaming,
}: ReasoningBlockProps) {
  const { t } = useI18n();

  return (
    <details className={styles.root}>
      <summary className={styles.trigger}>
        <SparkIcon
          className={`${styles.spark} ${streaming ? styles.sparkLive : ''}`}
        />
        <span className={styles.label}>
          {streaming ? t('chat.thinking') : t('chat.thought')}
        </span>
        <ChevronIcon className={styles.chevron} />
      </summary>
      <div className={styles.content}>
        <Markdown>{reasoning}</Markdown>
      </div>
    </details>
  );
});
