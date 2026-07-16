import { useEffect, useMemo, useRef } from 'react';
import type {
  ConnectionPhase,
  DaemonSessionControls,
} from '../../hooks/useDaemonSession.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import { deriveConversation } from '../../state/deriveConversation.ts';
import type { RunActivity } from '../../state/sessionReducer.ts';
import type { PublicMessage } from '../../types/wire.ts';
import { AskCard } from './AskCard.tsx';
import styles from './Chat.module.css';
import { Composer } from './Composer.tsx';
import {
  AssistantMessage,
  contentToText,
  UserMessage,
} from './MessageItem.tsx';
import { PlanApprovalCard } from './PlanApprovalCard.tsx';
import { WorkDetail } from './WorkDetail.tsx';

interface ChatProps {
  controls: DaemonSessionControls;
}

const STATUS_KEYS = {
  awaiting_first_prompt: 'chat.status.awaiting_first_prompt',
  idle: 'chat.status.idle',
  compacting: 'chat.status.compacting',
  running: 'chat.status.running',
  stopping: 'chat.status.stopping',
  closing: 'chat.status.closing',
  closed: 'chat.status.closed',
  offline: 'chat.status.offline',
} satisfies Record<string, import('../../i18n/translations.ts').TranslationKey>;

const CONNECTION_STATUS_KEYS = {
  idle: 'chat.connection.idle',
  connecting: 'chat.connection.connecting',
  preparing: 'chat.connection.preparing',
  ready: 'chat.status.idle',
  error: 'chat.connection.error',
} satisfies Record<ConnectionPhase, TranslationKey>;

const COMPOSER_PLACEHOLDER_KEYS = {
  idle: 'chat.composer.placeholderIdle',
  connecting: 'chat.composer.placeholderConnecting',
  preparing: 'chat.composer.placeholderPreparing',
  ready: 'chat.composer.placeholder',
  error: 'chat.composer.placeholderError',
} satisfies Record<ConnectionPhase, TranslationKey>;

export function Chat({ controls }: ChatProps) {
  const {
    state,
    connectionPhase,
    connectionError,
    retry,
    sendPrompt,
    stop,
    answerAsk,
    decidePlan,
  } = controls;
  const { t } = useI18n();
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const conversation = useMemo(
    () =>
      deriveConversation(
        state.history,
        state.draft,
        state.pendingUser,
        state.activeRun,
      ),
    [state.history, state.draft, state.pendingUser, state.activeRun],
  );

  // Auto-scroll to the bottom as content streams in, unless the user scrolled up.
  const stickToBottom = useRef(true);
  useEffect(() => {
    const el = scrollRef.current;
    if (el !== null && stickToBottom.current) {
      el.scrollTop = el.scrollHeight;
    }
  });

  const handleScroll = () => {
    const el = scrollRef.current;
    if (el === null) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    stickToBottom.current = atBottom;
  };

  const busy =
    state.status === 'running' ||
    state.status === 'stopping' ||
    state.status === 'compacting';
  const contextPct =
    state.contextUsage && state.contextUsage.max_tokens > 0
      ? Math.min(
          100,
          (state.contextUsage.used_tokens / state.contextUsage.max_tokens) *
            100,
        )
      : null;
  const waiting =
    connectionPhase === 'connecting' || connectionPhase === 'preparing';
  const displayStatus =
    connectionPhase === 'ready'
      ? t(STATUS_KEYS[state.status])
      : t(CONNECTION_STATUS_KEYS[connectionPhase]);
  const statusClass =
    connectionPhase === 'ready'
      ? state.status
      : connectionPhase === 'connecting' || connectionPhase === 'preparing'
        ? 'compacting'
        : 'offline';
  const visibleConnectionError = state.fatalError
    ? `${state.fatalError.code} — ${state.fatalError.message}`
    : connectionError;

  return (
    <section className={styles.chat}>
      <header className={styles.header}>
        <div className={styles.headerLeft}>
          <span
            className={`${styles.statusDot} ${styles[`status_${statusClass}`] ?? styles.status_idle}`}
          />
          <span className={styles.status}>{displayStatus}</span>
          {state.config && (
            <span className={styles.model}>{state.config.model}</span>
          )}
          <span
            className={`${styles.modeBadge} ${state.mode === 'plan' ? styles.modePlan : ''}`}
          >
            {state.mode}
          </span>
          {state.queuedRuns > 0 && (
            <span className={styles.queueHint}>
              {state.queuedRuns} {t('chat.queued')}
            </span>
          )}
        </div>
        <div className={styles.headerRight}>
          {contextPct !== null && (
            <div
              className={styles.context}
              title={t('chat.contextTitle', { pct: Math.round(contextPct) })}
            >
              <div className={styles.contextBar}>
                <div
                  className={`${styles.contextFill} ${contextPct > 85 ? styles.contextHigh : ''}`}
                  style={{ width: `${contextPct}%` }}
                />
              </div>
              <span className={styles.contextLabel}>
                {Math.round(contextPct)}%
              </span>
            </div>
          )}
          {busy && (
            <button type="button" className={styles.stopBtn} onClick={stop}>
              {t('chat.stop')}
            </button>
          )}
        </div>
      </header>

      {visibleConnectionError && (
        <div className={styles.errorBanner}>
          <span>{visibleConnectionError}</span>
          {connectionPhase === 'error' && (
            <button type="button" className={styles.retryBtn} onClick={retry}>
              {t('chat.connection.retry')}
            </button>
          )}
        </div>
      )}

      <div className={styles.scroll} ref={scrollRef} onScroll={handleScroll}>
        <div className={styles.transcript}>
          {conversation.items.length === 0 && connectionPhase === 'idle' && (
            <div className={styles.empty}>
              <h2 className={styles.emptyTitle}>{t('app.empty.title')}</h2>
              <p className={styles.emptyHint}>{t('app.empty.hint')}</p>
            </div>
          )}
          {waiting && conversation.items.length === 0 && (
            <div className={styles.empty}>
              <p className={styles.emptyHint}>
                {connectionPhase === 'connecting'
                  ? t('app.connecting')
                  : t('app.preparing')}
              </p>
            </div>
          )}
          {connectionPhase === 'error' && conversation.items.length === 0 && (
            <div className={styles.empty}>
              <p className={styles.emptyHint}>{t('chat.connection.error')}</p>
            </div>
          )}

          {conversation.items.map((item) => {
            if (item.kind === 'user') {
              return (
                <UserMessage
                  key={item.key}
                  message={item.message}
                  optimistic={item.optimistic}
                />
              );
            }
            if (item.kind === 'toolResult') {
              return (
                <ToolResultSummary key={item.key} message={item.message} />
              );
            }
            return (
              <div key={item.key} className={styles.assistantBlock}>
                {item.steps.length > 0 && (
                  <WorkDetail
                    steps={item.steps}
                    collapsed={item.collapsed}
                    runStatus={item.runStatus as RunActivity['status'] | null}
                    turnNumber={item.turnNumber}
                    errorMessage={item.errorMessage}
                  />
                )}
                <AssistantMessage
                  message={item.message}
                  draft={item.draft}
                  pending={
                    item.message === null && item.runStatus === 'running'
                  }
                />
              </div>
            );
          })}

          {state.pendingAsks.map((ask) => (
            <AskCard key={ask.ask_id} request={ask} onAnswer={answerAsk} />
          ))}
          {state.pendingPlanApprovals.map((approval) => (
            <PlanApprovalCard
              key={approval.approval_id}
              request={approval}
              onDecide={decidePlan}
            />
          ))}

          {state.notices.length > 0 && (
            <div className={styles.notices}>
              {state.notices.map((notice) => (
                <div key={`notice-${notice}`} className={styles.notice}>
                  {notice}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <Composer
        disabled={connectionPhase !== 'ready' || state.fatalError !== null}
        busy={busy}
        onSend={sendPrompt}
        onStop={stop}
        placeholder={t(COMPOSER_PLACEHOLDER_KEYS[connectionPhase])}
      />
    </section>
  );
}

function ToolResultSummary({ message }: { message: PublicMessage }) {
  const { t } = useI18n();
  const text = contentToText(message.content);
  const isError = message.tool_result_is_error;
  return (
    <details className={styles.toolResult}>
      <summary className={styles.toolResultSummary}>
        <span
          className={`${styles.toolResultDot} ${isError ? styles.toolResultError : ''}`}
        />
        {isError ? t('chat.toolResultError') : t('chat.toolResult')}
      </summary>
      <pre className={styles.toolResultBody}>
        {text || t('chat.toolResultEmpty')}
      </pre>
    </details>
  );
}
