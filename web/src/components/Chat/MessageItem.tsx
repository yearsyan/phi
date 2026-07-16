import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  AssistantDraft,
  PublicMessage,
  ToolCallDraft,
} from '../../types/wire.ts';
import { Markdown } from '../common/Markdown.tsx';
import styles from './MessageItem.module.css';

interface UserMessageProps {
  message: PublicMessage;
  optimistic?: boolean;
}

export function UserMessage({ message, optimistic }: UserMessageProps) {
  const text = contentToText(message.content);
  return (
    <div className={styles.userRow}>
      <div
        className={`${styles.userBubble} ${optimistic ? styles.optimistic : ''}`}
      >
        {text}
      </div>
    </div>
  );
}

interface AssistantMessageProps {
  message: PublicMessage | null;
  draft: AssistantDraft | null;
  pending?: boolean;
}

export function AssistantMessage({
  message,
  draft,
  pending,
}: AssistantMessageProps) {
  // Prefer the committed message text; fall back to the live draft.
  const { t } = useI18n();
  const committedText = message ? contentToText(message.content) : '';
  const draftText = draft ? draft.text : '';
  const text = committedText || draftText;

  return (
    <div className={styles.assistantRow}>
      <div className={styles.roleLabel}>{t('chat.assistant')}</div>
      {text.length > 0 ? (
        <Markdown>{text}</Markdown>
      ) : pending ? (
        <div className={styles.thinking}>
          <span className={styles.cursor} />
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
  );
}

function DraftToolCall({ toolCall }: { toolCall: ToolCallDraft }) {
  return (
    <div className={styles.draftTool}>
      <span className={styles.draftToolName}>{toolCall.name ?? 'tool'}</span>
      <span className={styles.draftToolArgs}>{toolCall.arguments || '…'}</span>
    </div>
  );
}

export function contentToText(content: PublicMessage['content']): string {
  if (content === null) return '';
  if (content.type === 'text') return content.value;
  return content.value
    .map((part) => (part.type === 'text' ? part.text : ''))
    .join('');
}
