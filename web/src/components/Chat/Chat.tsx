import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  ConnectionPhase,
  DaemonSessionControls,
} from '../../hooks/useDaemonSession.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import { deriveConversation } from '../../state/deriveConversation.ts';
import type {
  CapabilityMode,
  PublicMessage,
  ToolCall,
} from '../../types/wire.ts';
import {
  ArrowDownIcon,
  CompactIcon,
  GearIcon,
  MenuIcon,
  SparkIcon,
} from '../common/Icons.tsx';
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
  profileId: string;
  conversationKey?: string;
  onOpenSidebar: () => void;
  onOpenSettings: () => void;
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
} satisfies Record<string, TranslationKey>;

const CONNECTION_KEYS = {
  idle: 'chat.connection.idle',
  connecting: 'chat.connection.connecting',
  preparing: 'chat.connection.preparing',
  reconnecting: 'chat.connection.reconnecting',
  ready: 'chat.status.idle',
  error: 'chat.connection.error',
} satisfies Record<ConnectionPhase, TranslationKey>;

export function Chat({
  controls,
  profileId,
  conversationKey,
  onOpenSidebar,
  onOpenSettings,
}: ChatProps) {
  const {
    state,
    connectionPhase,
    connectionError,
    retry,
    sendPrompt,
    stop,
    answerAsk,
    decidePlan,
    setMode,
    setCapabilityMode,
    compact,
    clearNotice,
  } = controls;
  const { t } = useI18n();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);
  const [showJump, setShowJump] = useState(false);

  const conversation = useMemo(
    () =>
      deriveConversation(
        state.history,
        state.draft,
        state.pendingPrompts,
        state.activeRun,
      ),
    [state.history, state.draft, state.pendingPrompts, state.activeRun],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: the session identity resets the user's scroll intent
  useEffect(() => {
    stickToBottom.current = true;
    setShowJump(false);
  }, [state.sessionId, conversationKey]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element && stickToBottom.current) {
      element.scrollTop = element.scrollHeight;
    }
  });

  const busy =
    state.status === 'running' ||
    state.status === 'stopping' ||
    state.status === 'compacting';
  const canSend =
    connectionPhase === 'ready' &&
    state.fatalError === null &&
    !['closing', 'closed', 'offline'].includes(state.status);
  const canChangeSession =
    connectionPhase === 'ready' && !busy && state.fatalError === null;
  const canStop = state.activeRunId !== null && state.status === 'running';
  const contextPct =
    state.contextUsage && state.contextUsage.max_tokens > 0
      ? Math.min(
          100,
          (state.contextUsage.used_tokens / state.contextUsage.max_tokens) *
            100,
        )
      : null;

  const statusText =
    connectionPhase === 'ready'
      ? t(STATUS_KEYS[state.status])
      : t(CONNECTION_KEYS[connectionPhase]);
  const sessionTitle = state.sessionId
    ? `Session ${state.sessionId.slice(-6)}`
    : t('sidebar.newSession');
  const visibleError = state.fatalError
    ? `${state.fatalError.code}: ${state.fatalError.message}`
    : connectionError;
  const empty = conversation.items.length === 0;

  const onScroll = () => {
    const element = scrollRef.current;
    if (!element) return;
    const distance =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    stickToBottom.current = distance < 120;
    setShowJump(!stickToBottom.current);
  };

  const jumpToBottom = () => {
    const element = scrollRef.current;
    if (!element) return;
    stickToBottom.current = true;
    setShowJump(false);
    element.scrollTo({ top: element.scrollHeight, behavior: 'smooth' });
  };

  return (
    <section className={styles.chat}>
      <header className={styles.topbar}>
        <div className={styles.identity}>
          <button
            type="button"
            className={`${styles.iconButton} ${styles.menuButton}`}
            onClick={onOpenSidebar}
            aria-label={t('sidebar.sessions')}
          >
            <MenuIcon />
          </button>
          <div className={styles.sessionMark}>
            <SparkIcon />
          </div>
          <div className={styles.sessionIdentity}>
            <div className={styles.sessionTitle}>{sessionTitle}</div>
            <div className={styles.sessionMeta}>
              <span
                className={`${styles.statusDot} ${
                  connectionPhase === 'ready' && !busy
                    ? styles.statusReady
                    : connectionPhase === 'error'
                      ? styles.statusError
                      : styles.statusBusy
                }`}
              />
              <span>{statusText}</span>
              <span className={styles.metaDivider}>/</span>
              <span>{state.config?.model ?? profileId}</span>
              {state.agentProfile && (
                <>
                  <span className={styles.metaDivider}>/</span>
                  <span>
                    {state.agentProfile.agent_profile_id}@
                    {state.agentProfile.revision}
                  </span>
                </>
              )}
            </div>
          </div>
        </div>

        <div className={styles.controls}>
          {contextPct !== null && (
            <div
              className={styles.contextMeter}
              title={t('chat.contextTitle', { pct: Math.round(contextPct) })}
            >
              <span>{Math.round(contextPct)}%</span>
              <div className={styles.contextTrack}>
                <i
                  className={contextPct > 85 ? styles.contextHigh : ''}
                  style={{ width: `${contextPct}%` }}
                />
              </div>
            </div>
          )}

          <div className={styles.modeSwitch}>
            <button
              type="button"
              className={state.mode === 'default' ? styles.modeActive : ''}
              onClick={() => setMode('default')}
              disabled={!canChangeSession || state.mode === 'default'}
            >
              {t('chat.mode.default')}
            </button>
            <button
              type="button"
              className={state.mode === 'plan' ? styles.modeActive : ''}
              onClick={() => setMode('plan')}
              disabled={!canChangeSession || state.mode === 'plan'}
            >
              {t('chat.mode.plan')}
            </button>
          </div>

          <label
            className={styles.capabilityControl}
            title={t('chat.capability.title')}
          >
            <span>{t('chat.capability.label')}</span>
            <select
              aria-label={t('chat.capability.label')}
              value={state.capabilityMode}
              onChange={(event) =>
                setCapabilityMode(event.target.value as CapabilityMode)
              }
              disabled={!canChangeSession}
            >
              <option value="read_only">{t('chat.capability.readOnly')}</option>
              <option value="workspace_edit">
                {t('chat.capability.workspaceEdit')}
              </option>
              <option value="full_access">
                {t('chat.capability.fullAccess')}
              </option>
            </select>
          </label>

          <button
            type="button"
            className={styles.actionButton}
            onClick={() => compact()}
            disabled={!canChangeSession || state.sessionId === null}
            title={t('chat.compact')}
          >
            <CompactIcon />
            <span>{t('chat.compact')}</span>
          </button>
          <button
            type="button"
            className={styles.iconButton}
            onClick={onOpenSettings}
            aria-label={t('sidebar.settings')}
          >
            <GearIcon />
          </button>
        </div>
      </header>

      {visibleError && (
        <div className={styles.connectionBanner} role="alert">
          <span>{visibleError}</span>
          {connectionPhase === 'error' && (
            <button type="button" onClick={retry}>
              {t('chat.connection.retry')}
            </button>
          )}
        </div>
      )}

      <div className={styles.scroll} ref={scrollRef} onScroll={onScroll}>
        <div className={styles.timeline}>
          {empty && connectionPhase === 'ready' && (
            <Welcome onSend={sendPrompt} />
          )}
          {empty &&
            (connectionPhase === 'connecting' ||
              connectionPhase === 'preparing' ||
              connectionPhase === 'reconnecting') && (
              <ConnectionPlaceholder phase={connectionPhase} />
            )}
          {empty && connectionPhase === 'idle' && (
            <div className={styles.centerState}>
              <SparkIcon />
              <h2>{t('app.empty.title')}</h2>
              <p>{t('app.empty.hint')}</p>
            </div>
          )}

          {conversation.items.map((item) => {
            if (item.kind === 'user') {
              return (
                <UserMessage
                  key={item.key}
                  message={item.message}
                  pending={item.pending}
                />
              );
            }
            if (item.kind === 'assistant') {
              return (
                <AssistantMessage
                  key={item.key}
                  message={item.message}
                  draft={item.draft}
                  pending={item.pending}
                />
              );
            }
            if (item.kind === 'activity') {
              return <WorkDetail key={item.key} run={item.run} />;
            }
            return (
              <ToolGroup
                key={item.key}
                calls={item.calls}
                results={item.results}
              />
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

          {state.notices.slice(-4).map((notice, index, visible) => {
            const sourceIndex = state.notices.length - visible.length + index;
            return (
              <div
                className={styles.notice}
                key={`${sourceIndex}-${notice}`}
                role="status"
              >
                <span>{notice}</span>
                <button
                  type="button"
                  onClick={() => clearNotice(sourceIndex)}
                  aria-label={t('chat.notice.dismiss')}
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>
      </div>

      {showJump && (
        <button
          type="button"
          className={styles.jumpButton}
          onClick={jumpToBottom}
        >
          <ArrowDownIcon />
          {t('chat.jumpToBottom')}
        </button>
      )}

      <Composer
        key={conversationKey ?? state.sessionId ?? `prepared:${profileId}`}
        disabled={!canSend}
        busy={busy}
        canStop={canStop}
        queuedCount={state.queuedRuns}
        onSend={sendPrompt}
        onStop={stop}
      />
    </section>
  );
}

function Welcome({ onSend }: { onSend: (text: string) => boolean }) {
  const { t } = useI18n();
  const suggestions = [
    ['chat.welcome.inspect', 'chat.welcome.inspectPrompt'],
    ['chat.welcome.fix', 'chat.welcome.fixPrompt'],
    ['chat.welcome.explain', 'chat.welcome.explainPrompt'],
  ] as const satisfies ReadonlyArray<readonly [TranslationKey, TranslationKey]>;

  return (
    <div className={styles.welcome}>
      <div className={styles.welcomeMark}>φ</div>
      <p className={styles.eyebrow}>{t('chat.welcome.eyebrow')}</p>
      <h1>{t('chat.welcome.title')}</h1>
      <p className={styles.welcomeCopy}>{t('chat.welcome.copy')}</p>
      <div className={styles.suggestions}>
        {suggestions.map(([label, prompt]) => (
          <button type="button" key={label} onClick={() => onSend(t(prompt))}>
            <span>{t(label)}</span>
            <small>{t(prompt)}</small>
          </button>
        ))}
      </div>
    </div>
  );
}

function ConnectionPlaceholder({ phase }: { phase: ConnectionPhase }) {
  const { t } = useI18n();
  return (
    <div className={styles.centerState} aria-live="polite">
      <span className={styles.loader} />
      <h2>{t(CONNECTION_KEYS[phase])}</h2>
      <p>{t('chat.connection.waitHint')}</p>
    </div>
  );
}

function ToolGroup({
  calls,
  results,
}: {
  calls: ToolCall[];
  results: PublicMessage[];
}) {
  const { t } = useI18n();
  return (
    <details className={styles.toolGroup}>
      <summary>
        <TerminalSummaryDot error={results.some(isToolError)} />
        <span>
          {calls.length > 0
            ? t('chat.toolGroup', { count: calls.length })
            : t('chat.toolResult')}
        </span>
      </summary>
      <div className={styles.toolGroupBody}>
        {(calls.length > 0 ? calls : [null]).map((call, index) => {
          const result = call
            ? results.find((entry) => entry.tool_call_id === call.id)
            : results[index];
          return (
            <div className={styles.toolHistoryRow} key={call?.id ?? index}>
              <div className={styles.toolHistoryHead}>
                <strong>{call?.name ?? t('chat.toolResult')}</strong>
                {result?.tool_result_is_error && (
                  <span>{t('chat.activity.toolFailed')}</span>
                )}
              </div>
              {call && <pre>{safeStringify(call.arguments)}</pre>}
              {result && <pre>{contentToText(result.content)}</pre>}
            </div>
          );
        })}
      </div>
    </details>
  );
}

function TerminalSummaryDot({ error }: { error: boolean }) {
  return (
    <span
      className={`${styles.toolGroupDot} ${error ? styles.toolGroupError : ''}`}
    />
  );
}

function isToolError(message: PublicMessage): boolean {
  return message.tool_result_is_error;
}

function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}
