import { useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { AskUserAnswer, AskUserRequest } from '../../types/wire.ts';
import styles from './AskCard.module.css';

interface AskCardProps {
  request: AskUserRequest;
  onAnswer: (askId: string, answers: AskUserAnswer[]) => void;
}

interface QuestionDraft {
  selected: Set<string>;
  custom: string;
}

const emptyDraft = (): QuestionDraft => ({ selected: new Set(), custom: '' });

/**
 * Inline card for an `askuser_requested` interaction. The model asks 1–3
 * questions; the user picks explicit options (single- or multi-select) and may
 * supply a custom "Other" reply. Submitting answers all questions at once.
 */
export function AskCard({ request, onAnswer }: AskCardProps) {
  const { t } = useI18n();
  const [drafts, setDrafts] = useState<QuestionDraft[]>(() =>
    request.questions.map(() => emptyDraft()),
  );
  const [showOther, setShowOther] = useState<boolean[]>(() =>
    request.questions.map(() => false),
  );

  const toggleOption = (questionIndex: number, label: string) => {
    setDrafts((prev) => {
      const next = prev.map((draft) => ({
        selected: new Set(draft.selected),
        custom: draft.custom,
      }));
      const question = request.questions[questionIndex];
      if (question === undefined) return prev;
      const selected = next[questionIndex]?.selected ?? new Set<string>();
      if (question.multi_select) {
        if (selected.has(label)) {
          selected.delete(label);
        } else {
          selected.add(label);
        }
      } else {
        selected.clear();
        selected.add(label);
      }
      next[questionIndex] = {
        selected,
        custom: next[questionIndex]?.custom ?? '',
      };
      return next;
    });
  };

  const setCustom = (questionIndex: number, value: string) => {
    setDrafts((prev) => {
      const target = prev[questionIndex];
      if (target === undefined) return prev;
      const next = prev.map((draft) => ({
        selected: new Set(draft.selected),
        custom: draft.custom,
      }));
      next[questionIndex] = {
        selected: new Set(target.selected),
        custom: value,
      };
      return next;
    });
  };

  const toggleOther = (questionIndex: number) => {
    setShowOther((prev) => {
      const next = [...prev];
      next[questionIndex] = !next[questionIndex];
      return next;
    });
  };

  const canSubmit = request.questions.every((_question, index) => {
    const draft = drafts[index];
    if (draft === undefined) return false;
    return draft.selected.size > 0 || draft.custom.trim().length > 0;
  });

  const submit = () => {
    const answers: AskUserAnswer[] = request.questions.map(
      (question, index) => {
        const draft = drafts[index] ?? emptyDraft();
        const selected = Array.from(draft.selected).filter((label) =>
          question.options.some((option) => option.label === label),
        );
        const custom =
          draft.custom.trim().length > 0 ? draft.custom.trim() : null;
        return {
          question_index: index,
          selected_options: selected,
          custom_text: custom,
        };
      },
    );
    onAnswer(request.ask_id, answers);
  };

  return (
    <div className={styles.card}>
      <div className={styles.header}>
        <span className={styles.badge}>{t('ask.badge')}</span>
        <span className={styles.title}>{t('ask.title')}</span>
      </div>
      <div className={styles.questions}>
        {request.questions.map((question, questionIndex) => {
          const draft = drafts[questionIndex] ?? emptyDraft();
          const hasPreview = question.options.some((option) => option.preview);
          const previewOption = hasPreview
            ? question.options.find((option) =>
                draft.selected.has(option.label),
              )
            : undefined;
          return (
            <div key={`q-${question.header}`} className={styles.question}>
              <div className={styles.questionHeader}>
                <span className={styles.questionLabel}>{question.header}</span>
                <span className={styles.multiHint}>
                  {question.multi_select
                    ? t('ask.multiSelect')
                    : t('ask.singleSelect')}
                </span>
              </div>
              <div className={styles.questionText}>{question.question}</div>
              <div
                className={`${styles.options} ${hasPreview ? styles.optionsGrid : ''}`}
              >
                <div className={styles.optionList}>
                  {question.options.map((option) => {
                    const selected = draft.selected.has(option.label);
                    return (
                      <button
                        type="button"
                        key={option.label}
                        className={`${styles.option} ${selected ? styles.optionSelected : ''}`}
                        onClick={() =>
                          toggleOption(questionIndex, option.label)
                        }
                      >
                        <div className={styles.optionMain}>
                          <span className={styles.optionLabel}>
                            {option.label}
                          </span>
                          {option.description && (
                            <span className={styles.optionDesc}>
                              {option.description}
                            </span>
                          )}
                        </div>
                      </button>
                    );
                  })}
                  <button
                    type="button"
                    className={`${styles.option} ${showOther[questionIndex] ? styles.optionSelected : ''}`}
                    onClick={() => toggleOther(questionIndex)}
                  >
                    <span className={styles.optionLabel}>{t('ask.other')}</span>
                  </button>
                </div>
                {hasPreview && (
                  <div className={styles.previewWrap}>
                    {previewOption?.preview ? (
                      <pre className={styles.preview}>
                        {previewOption.preview}
                      </pre>
                    ) : (
                      <div className={styles.previewPlaceholder}>
                        {t('ask.previewPlaceholder')}
                      </div>
                    )}
                  </div>
                )}
              </div>
              {showOther[questionIndex] && (
                <input
                  type="text"
                  className={styles.customInput}
                  placeholder={t('ask.otherPlaceholder')}
                  value={draft.custom}
                  onChange={(event) =>
                    setCustom(questionIndex, event.target.value)
                  }
                />
              )}
            </div>
          );
        })}
      </div>
      <div className={styles.actions}>
        <button
          type="button"
          className={styles.submitBtn}
          onClick={submit}
          disabled={!canSubmit}
        >
          {t('ask.submit')}
        </button>
      </div>
    </div>
  );
}
