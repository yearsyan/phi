import { useEffect, useId, useRef, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { AskUserAnswer, AskUserRequest } from '../../types/wire.ts';
import styles from './AskCard.module.css';

interface AskCardProps {
  request: AskUserRequest;
  onAnswer: (askId: string, answers: AskUserAnswer[]) => boolean;
}

interface QuestionDraft {
  selected: Set<string>;
  custom: string;
}

const emptyDraft = (): QuestionDraft => ({ selected: new Set(), custom: '' });

/**
 * Interactive card for an `askuser_requested` interaction. The model asks 1–3
 * questions; the user picks explicit options (single- or multi-select) and may
 * supply a custom "Other" reply. Submitting answers all questions at once.
 */
export function AskCard({ request, onAnswer }: AskCardProps) {
  const { t } = useI18n();
  const titleId = useId();
  const [drafts, setDrafts] = useState<QuestionDraft[]>(() =>
    request.questions.map(() => emptyDraft()),
  );
  const [showOther, setShowOther] = useState<boolean[]>(() =>
    request.questions.map(() => false),
  );
  const [submitting, setSubmitting] = useState(false);
  const retryTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (retryTimer.current !== null) {
        window.clearTimeout(retryTimer.current);
      }
    },
    [],
  );

  const toggleOption = (questionIndex: number, label: string) => {
    const question = request.questions[questionIndex];
    if (question === undefined) return;
    if (!question.multiSelect) {
      setShowOther((current) =>
        current.map((visible, index) =>
          index === questionIndex ? false : visible,
        ),
      );
    }
    setDrafts((prev) => {
      const next = prev.map((draft) => ({
        selected: new Set(draft.selected),
        custom: draft.custom,
      }));
      const selected = next[questionIndex]?.selected ?? new Set<string>();
      if (question.multiSelect) {
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
        custom: question.multiSelect ? (next[questionIndex]?.custom ?? '') : '',
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
        selected:
          questionAt(request, questionIndex)?.multiSelect === false &&
          value.trim().length > 0
            ? new Set()
            : new Set(target.selected),
        custom: value,
      };
      return next;
    });
  };

  const toggleOther = (questionIndex: number) => {
    const visible = !showOther[questionIndex];
    setShowOther((prev) =>
      prev.map((current, index) =>
        index === questionIndex ? visible : current,
      ),
    );
    if (!visible) {
      setCustom(questionIndex, '');
    } else if (questionAt(request, questionIndex)?.multiSelect === false) {
      setDrafts((drafts) =>
        drafts.map((draft, index) =>
          index === questionIndex
            ? { selected: new Set(), custom: draft.custom }
            : draft,
        ),
      );
    }
  };

  const canSubmit = request.questions.every((question, index) => {
    const draft = drafts[index];
    if (draft === undefined) return false;
    const count =
      draft.selected.size + (draft.custom.trim().length > 0 ? 1 : 0);
    return question.multiSelect ? count > 0 : count === 1;
  });

  const submit = () => {
    const answers: AskUserAnswer[] = request.questions.map(
      (question, index) => {
        const draft = drafts[index] ?? emptyDraft();
        const custom =
          draft.custom.trim().length > 0 ? draft.custom.trim() : null;
        const selected = Array.from(draft.selected).filter((label) =>
          question.options.some((option) => option.label === label),
        );
        return {
          question_index: index,
          selected_options:
            !question.multiSelect && custom !== null ? [] : selected,
          custom_text: custom,
        };
      },
    );
    if (onAnswer(request.ask_id, answers)) {
      setSubmitting(true);
      retryTimer.current = window.setTimeout(() => {
        retryTimer.current = null;
        setSubmitting(false);
      }, 2_000);
    }
  };

  return (
    <section className={styles.card} role="dialog" aria-labelledby={titleId}>
      <div className={styles.header}>
        <span className={styles.badge}>{t('ask.badge')}</span>
        <span className={styles.title} id={titleId}>
          {t('ask.title')}
        </span>
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
                  {question.multiSelect
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
          disabled={!canSubmit || submitting}
        >
          {submitting ? t('ask.submitting') : t('ask.submit')}
        </button>
      </div>
    </section>
  );
}

function questionAt(request: AskUserRequest, index: number) {
  return request.questions[index];
}
