import {
  type CSSProperties,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type {
  ConnectionPhase,
  DaemonSessionControls,
} from '../../hooks/useDaemonSession.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import { deriveTimeline } from '../../state/timeline.ts';
import type { ForkPosition, PublicProviderConfig } from '../../types/wire.ts';
import { FolderIcon, GearIcon, MenuIcon, SparkIcon } from '../common/Icons.tsx';
import { AskCard } from './AskCard.tsx';
import styles from './Chat.module.css';
import { Composer } from './Composer.tsx';
import { Timeline } from './Timeline.tsx';
import { WorkspacePicker, workspaceName } from './WorkspacePicker.tsx';

interface ChatProps {
  controls: DaemonSessionControls;
  authKey: string;
  profileId: string;
  providerProfiles: PublicProviderConfig[];
  conversationKey?: string;
  onFork: (messageIndex: number, position: ForkPosition) => Promise<void>;
  onSelectProvider: (profileId: string) => void;
  onSelectWorkspace: (workspace: string) => void;
  onOpenSidebar: () => void;
  onOpenSettings: () => void;
}

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
  authKey,
  profileId,
  providerProfiles,
  conversationKey,
  onFork,
  onSelectProvider,
  onSelectWorkspace,
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
    setModel,
    setReasoningEffort,
    setCapabilityMode,
    compact,
    clearNotice,
  } = controls;
  const { t } = useI18n();
  const interactionDockRef = useRef<HTMLDivElement | null>(null);
  const [interactionDockHeight, setInteractionDockHeight] = useState(0);

  const timeline = useMemo(
    () =>
      deriveTimeline(
        state.history,
        state.draft,
        state.pendingPrompts,
        state.activeRun,
        state.compactions,
      ),
    [
      state.history,
      state.draft,
      state.pendingPrompts,
      state.activeRun,
      state.compactions,
    ],
  );

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
  const canCompact =
    canChangeSession && state.sessionId !== null && state.history.length > 0;
  const canFork =
    connectionPhase === 'ready' &&
    state.sessionId !== null &&
    state.fatalError === null &&
    !['compacting', 'closing', 'closed', 'offline'].includes(state.status);
  const canStop = state.activeRunId !== null && state.status === 'running';

  const sessionTitle =
    state.title ??
    (state.sessionId
      ? `Session ${state.sessionId.slice(-6)}`
      : t('sidebar.newSession'));
  const visibleError = state.fatalError
    ? `${state.fatalError.code}: ${state.fatalError.message}`
    : connectionError;
  const empty = timeline.items.length === 0;
  const isNewSession =
    empty && state.sessionId === null && connectionPhase === 'ready';
  const hasPanels = state.pendingAsks.length > 0 || state.notices.length > 0;
  const activeProfileId = state.profileId ?? profileId;
  const displayedModel =
    state.config?.model ??
    providerProfiles.find((profile) => profile.profile_id === activeProfileId)
      ?.model ??
    activeProfileId;

  useLayoutEffect(() => {
    const dock = interactionDockRef.current;
    if (isNewSession || dock === null) {
      setInteractionDockHeight(0);
      return;
    }

    const measure = () => {
      setInteractionDockHeight(Math.ceil(dock.getBoundingClientRect().height));
    };
    measure();

    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(dock);
    return () => observer.disconnect();
  }, [isNewSession]);

  const chatStyle = {
    '--interaction-dock-height': `${interactionDockHeight}px`,
  } as CSSProperties;

  const composer = (variant: 'default' | 'welcome' = 'default') => (
    <Composer
      key={conversationKey ?? state.sessionId ?? `prepared:${profileId}`}
      variant={variant}
      disabled={!canSend}
      busy={busy}
      canStop={canStop}
      canConfigure={canChangeSession}
      sessionActivated={state.sessionId !== null}
      canCompact={canCompact}
      queuedCount={state.queuedRuns}
      capabilityMode={state.capabilityMode}
      profileId={activeProfileId}
      providerProfiles={providerProfiles}
      model={displayedModel}
      reasoningEffort={state.config?.reasoning_effort ?? null}
      usage={state.usage}
      skills={state.skills}
      onSend={sendPrompt}
      onStop={stop}
      onSetCapabilityMode={setCapabilityMode}
      onSelectProvider={onSelectProvider}
      onSetModel={setModel}
      onSetReasoningEffort={setReasoningEffort}
      onCompact={compact}
    />
  );

  return (
    <section className={styles.chat} style={chatStyle}>
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
          <div className={styles.sessionTitle} title={sessionTitle}>
            {sessionTitle}
          </div>
          {state.workspace && (
            <div className={styles.workspace} title={state.workspace}>
              <FolderIcon />
              <span className={styles.workspaceName} aria-hidden="true">
                {workspaceName(state.workspace)}
              </span>
              <span className={styles.screenReaderOnly}>
                {t('chat.workspace')}: {state.workspace}
              </span>
            </div>
          )}
        </div>

        <div className={styles.controls}>
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

      {isNewSession ? (
        <div className={styles.newSessionStage}>
          <div className={styles.newSessionPrompt}>
            <WorkspacePicker
              authKey={authKey}
              workspace={state.workspace}
              disabled={!canChangeSession}
              onSelect={onSelectWorkspace}
            />
            {composer('welcome')}
          </div>
        </div>
      ) : empty ? (
        <div className={styles.stage}>
          {(connectionPhase === 'connecting' ||
            connectionPhase === 'preparing' ||
            connectionPhase === 'reconnecting') && (
            <ConnectionPlaceholder phase={connectionPhase} />
          )}
          {connectionPhase === 'idle' && (
            <div className={styles.centerState}>
              <SparkIcon />
              <h2>{t('app.empty.title')}</h2>
              <p>{t('app.empty.hint')}</p>
            </div>
          )}
        </div>
      ) : (
        <Timeline
          items={timeline.items}
          bottomInset={interactionDockHeight}
          canFork={canFork}
          onFork={onFork}
          conversationKey={
            conversationKey ?? state.sessionId ?? `prepared:${profileId}`
          }
        />
      )}

      {!isNewSession && (
        <div ref={interactionDockRef} className={styles.interactionDock}>
          {hasPanels && (
            <div className={styles.panels}>
              {state.pendingAsks.map((ask) => (
                <AskCard key={ask.ask_id} request={ask} onAnswer={answerAsk} />
              ))}
              {state.notices.slice(-4).map((notice, index, visible) => {
                const sourceIndex =
                  state.notices.length - visible.length + index;
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
          )}

          {composer()}
        </div>
      )}
    </section>
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
