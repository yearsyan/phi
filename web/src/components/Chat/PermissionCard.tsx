import { useEffect, useId, useRef, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import type {
  CapabilityMode,
  ToolEffect,
  ToolPermissionDecision,
  ToolPermissionPrompt,
  ToolPermissionRule,
} from '../../types/wire.ts';
import styles from './PermissionCard.module.css';

interface PermissionCardProps {
  request: ToolPermissionPrompt;
  onDecision: (
    permissionId: string,
    decision: ToolPermissionDecision,
  ) => boolean;
}

const MODE_KEYS = {
  read_only: 'chat.capability.readOnly',
  workspace_edit: 'chat.capability.workspaceEdit',
  full_access: 'chat.capability.fullAccess',
} satisfies Record<CapabilityMode, TranslationKey>;

const EFFECT_KEYS = {
  read_only: 'permission.effect.readOnly',
  internal: 'permission.effect.internal',
  workspace_write: 'permission.effect.workspaceWrite',
  external_side_effect: 'permission.effect.external',
} satisfies Record<ToolEffect, TranslationKey>;

export function PermissionCard({ request, onDecision }: PermissionCardProps) {
  const { t } = useI18n();
  const titleId = useId();
  const [suggestionIndex, setSuggestionIndex] = useState(0);
  const [submitting, setSubmitting] = useState(false);
  const retryTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (retryTimer.current !== null) window.clearTimeout(retryTimer.current);
    },
    [],
  );

  const decide = (decision: ToolPermissionDecision) => {
    if (!onDecision(request.permission_id, decision)) return;
    setSubmitting(true);
    retryTimer.current = window.setTimeout(() => {
      retryTimer.current = null;
      setSubmitting(false);
    }, 2_000);
  };
  const selectedRule = request.suggestions[suggestionIndex];

  return (
    <section className={styles.card} role="dialog" aria-labelledby={titleId}>
      <div className={styles.header}>
        <span className={styles.badge}>{t('permission.badge')}</span>
        <span className={styles.title} id={titleId}>
          {t('permission.title')}
        </span>
      </div>
      <div className={styles.body}>
        <div className={styles.summary}>
          {t('permission.summary', { tool: request.call.name })}
        </div>
        <pre className={styles.command}>{permissionTarget(request)}</pre>
        <div className={styles.metadata}>
          <span>
            {t('permission.effect')}: {t(EFFECT_KEYS[request.effect])}
          </span>
          <span>
            {t('permission.mode')}: {t(MODE_KEYS[request.capability_mode])}
          </span>
        </div>
        {request.suggestions.length > 1 && (
          <label className={styles.rulePicker}>
            <span>{t('permission.rule')}</span>
            <select
              value={suggestionIndex}
              onChange={(event) =>
                setSuggestionIndex(Number(event.target.value))
              }
              disabled={submitting}
            >
              {request.suggestions.map((rule, index) => (
                <option key={formatRule(rule)} value={index}>
                  {formatRule(rule)}
                </option>
              ))}
            </select>
          </label>
        )}
        {selectedRule && request.suggestions.length === 1 && (
          <div className={styles.rulePreview}>
            {t('permission.rule')}: <code>{formatRule(selectedRule)}</code>
          </div>
        )}
      </div>
      <div className={styles.actions}>
        <button
          type="button"
          className={styles.denyButton}
          disabled={submitting}
          onClick={() => decide({ type: 'deny' })}
        >
          {t('permission.deny')}
        </button>
        <div className={styles.allowActions}>
          <button
            type="button"
            className={styles.secondaryButton}
            disabled={submitting}
            onClick={() => decide({ type: 'allow_once' })}
          >
            {t('permission.allowOnce')}
          </button>
          {selectedRule && (
            <button
              type="button"
              className={styles.allowButton}
              disabled={submitting}
              onClick={() =>
                decide({ type: 'allow_for_session', rule: selectedRule })
              }
            >
              {t('permission.allowSession')}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}

function permissionTarget(request: ToolPermissionPrompt): string {
  const argumentsValue = request.call.arguments;
  if (
    request.call.name === 'bash' &&
    typeof argumentsValue === 'object' &&
    argumentsValue !== null &&
    'command' in argumentsValue &&
    typeof argumentsValue.command === 'string'
  ) {
    return argumentsValue.command;
  }
  return JSON.stringify(argumentsValue, null, 2) ?? String(argumentsValue);
}

function formatRule(rule: ToolPermissionRule): string {
  return rule.pattern === undefined
    ? rule.tool_name
    : `${rule.tool_name}(${rule.pattern})`;
}
