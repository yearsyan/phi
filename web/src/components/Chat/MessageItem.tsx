import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { PendingPrompt } from '../../state/sessionReducer.ts';
import type {
  AssistantDraft,
  PublicMessage,
  ToolCallDraft,
} from '../../types/wire.ts';
import { SparkIcon } from '../common/Icons.tsx';
import { Markdown } from '../common/Markdown.tsx';
import styles from './MessageItem.module.css';

interface UserMessageProps {
  message: PublicMessage;
  pending: PendingPrompt | null;
}

export function UserMessage({ message, pending }: UserMessageProps) {
  const { t } = useI18n();
  const text = contentToText(message.content);
  const pendingLabel =
    pending?.status === 'queued'
      ? t('chat.prompt.queued', { position: pending.queuePosition ?? '—' })
      : pending
        ? t('chat.prompt.sending')
        : null;

  return (
    <article className={styles.userRow}>
      <div className={styles.userContent}>
        <div className={styles.userBubble}>{text}</div>
        {pendingLabel && (
          <div className={styles.userMeta} aria-live="polite">
            <span className={styles.pendingDot} />
            {pendingLabel}
          </div>
        )}
      </div>
    </article>
  );
}

interface AssistantMessageProps {
  message: PublicMessage | null;
  draft: AssistantDraft | null;
  pending: boolean;
}

export function AssistantMessage({
  message,
  draft,
  pending,
}: AssistantMessageProps) {
  const { t } = useI18n();
  const committedText = message ? contentToText(message.content) : '';
  const draftText = draft?.text ?? '';
  const text = committedText || draftText;

  return (
    <article className={styles.assistantRow}>
      <div className={styles.assistantAvatar}>
        <SparkIcon />
      </div>
      <div className={styles.assistantContent}>
        <div className={styles.assistantLabel}>Phi</div>
        {text.length > 0 ? (
          <Markdown>{text}</Markdown>
        ) : pending ? (
          <div className={styles.thinking} aria-live="polite">
            <span>{t('chat.thinking')}</span>
            <span className={styles.thinkingDots} aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
          </div>
        ) : null}

        {draft !== null && draft.tool_calls.length > 0 && (
          <div className={styles.draftTools}>
            {draft.tool_calls.map((toolCall) => (
              <DraftToolCall key={toolCall.index} toolCall={toolCall} />
            ))}
          </div>
        )}
      </div>
    </article>
  );
}

function DraftToolCall({ toolCall }: { toolCall: ToolCallDraft }) {
  return (
    <div className={styles.draftTool}>
      <span className={styles.draftToolPulse} />
      <span className={styles.draftToolName}>{toolCall.name ?? 'tool'}</span>
      <span className={styles.draftToolArgs}>{toolCall.arguments || '…'}</span>
    </div>
  );
}

export function contentToText(
  content: PublicMessage['content'] | undefined,
): string {
  if (content === null || content === undefined) return '';
  if (content.type === 'text') return content.value;
  return content.value
    .map((part) => {
      if (part.type === 'text') return part.text;
      if (part.type === 'document') return `[${part.document.filename}]`;
      return '[image]';
    })
    .filter(Boolean)
    .join('\n');
}
