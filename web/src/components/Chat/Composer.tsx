import { useEffect, useRef, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import { SendIcon, StopIcon } from '../common/Icons.tsx';
import styles from './Composer.module.css';

interface ComposerProps {
  disabled: boolean;
  busy: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
  placeholder?: string;
}

export function Composer({
  disabled,
  busy,
  onSend,
  onStop,
  placeholder,
}: ComposerProps) {
  const [value, setValue] = useState('');
  const { t } = useI18n();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Auto-grow the textarea whenever its value changes.
  // biome-ignore lint/correctness/useExhaustiveDependencies: value is the trigger; the body only touches the ref
  useEffect(() => {
    const el = textareaRef.current;
    if (el === null) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  }, [value]);

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed || disabled) return;
    onSend(trimmed);
    setValue('');
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      event.key === 'Enter' &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      submit();
    }
  };

  return (
    <div className={styles.composer}>
      <div className={styles.row}>
        <textarea
          ref={textareaRef}
          className={styles.textarea}
          value={value}
          placeholder={placeholder ?? t('chat.composer.placeholder')}
          disabled={disabled}
          rows={1}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        {busy ? (
          <button
            type="button"
            className={styles.stopBtn}
            onClick={onStop}
            title={t('chat.composer.stopTitle')}
          >
            <StopIcon /> {t('chat.stop')}
          </button>
        ) : (
          <button
            type="button"
            className={styles.sendBtn}
            onClick={submit}
            disabled={disabled || value.trim().length === 0}
            title={t('chat.composer.sendTitle')}
          >
            <SendIcon />
            <span className={styles.sendLabel}>{t('chat.composer.send')}</span>
          </button>
        )}
      </div>
    </div>
  );
}
