import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import styles from './App.module.css';
import {
  isConfigured,
  readDaemonConfig,
  writeAgentProfileId,
  writeAuthKey,
  writeCapabilityMode,
  writeProfileId,
} from './api/config.ts';
import { forkSession, listProviders } from './api/http.ts';
import { Chat } from './components/Chat/Chat.tsx';
import { ScheduledTasksPage } from './components/ScheduledTasks/ScheduledTasksPage.tsx';
import { SettingsModal } from './components/Settings/SettingsModal.tsx';
import { Sidebar } from './components/Sidebar/Sidebar.tsx';
import {
  type SessionTarget,
  useDaemonSession,
} from './hooks/useDaemonSession.ts';
import { useSessionList } from './hooks/useSessionList.ts';
import { initialTheme, useTheme } from './hooks/useTheme.ts';
import { I18nProvider, useI18n } from './i18n/I18nProvider.tsx';
import { LOCALES, type Locale } from './i18n/translations.ts';
import { readLocale, type Theme, writeLocale } from './prefs.ts';
import type {
  CapabilityMode,
  ForkPosition,
  PublicProviderConfig,
} from './types/wire.ts';

type Selection =
  | { kind: 'none' }
  | { kind: 'new'; instanceId: number; workspace?: string }
  | { kind: 'scheduled_tasks' }
  | { kind: 'session'; sessionId: string };

function selectionToTarget(
  selection: Selection,
  profileId: string,
  agentProfileId: string,
  capabilityMode: CapabilityMode | null,
): SessionTarget | null {
  if (selection.kind === 'none') return null;
  if (selection.kind === 'scheduled_tasks') return null;
  if (selection.kind === 'new') {
    return {
      kind: 'new',
      profileId,
      agentProfileId: agentProfileId.trim() || undefined,
      capabilityMode: capabilityMode ?? undefined,
      workspace: selection.workspace,
      instanceId: selection.instanceId,
    };
  }
  return { kind: 'attach', sessionId: selection.sessionId };
}

function App() {
  const [initialLocale] = useState<Locale>(() => readLocale());
  const [themeState] = useState(() => initialTheme());

  return (
    <I18nProvider
      initialLocale={initialLocale}
      onChange={(locale) => writeLocale(locale)}
    >
      <AppShell initialTheme={themeState} />
    </I18nProvider>
  );
}

function AppShell({ initialTheme }: { initialTheme: Theme }) {
  const { locale, setLocale } = useI18n();
  const { theme, toggle: toggleTheme } = useTheme(initialTheme);
  const nextSessionInstance = useRef(1);

  const [config, setConfig] = useState(() => readDaemonConfig());
  const [selection, setSelection] = useState<Selection>(() =>
    isConfigured(config) ? { kind: 'new', instanceId: 1 } : { kind: 'none' },
  );
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [providerProfiles, setProviderProfiles] = useState<
    PublicProviderConfig[]
  >([]);
  const providerAuthKey = config.authKey.trim();

  useEffect(() => {
    if (!isConfigured(config)) setSettingsOpen(true);
  }, [config]);

  useEffect(() => {
    let cancelled = false;
    if (!providerAuthKey) {
      setProviderProfiles([]);
      return;
    }
    void listProviders(providerAuthKey)
      .then((response) => {
        if (!cancelled) setProviderProfiles(response.providers);
      })
      .catch(() => {
        if (!cancelled) setProviderProfiles([]);
      });
    return () => {
      cancelled = true;
    };
  }, [providerAuthKey]);

  const target = useMemo(
    () =>
      selectionToTarget(
        selection,
        config.profileId,
        config.agentProfileId,
        config.capabilityMode,
      ),
    [selection, config.profileId, config.agentProfileId, config.capabilityMode],
  );
  const controls = useDaemonSession(config.authKey, target);
  const {
    workspaces,
    loading: sessionsLoading,
    error: listError,
    refresh: refreshSessions,
    setPinned: setSessionPinned,
    deleteSession,
  } = useSessionList(config.authKey, isConfigured(config));

  const { sessionListRevision } = controls;
  useEffect(() => {
    if (sessionListRevision > 0) void refreshSessions();
  }, [sessionListRevision, refreshSessions]);

  const liveSessionId = controls.state.sessionId;
  const visibleWorkspaces = useMemo(
    () =>
      workspaces.map((workspace) => ({
        ...workspace,
        sessions: workspace.sessions.map((session) =>
          session.session_id === liveSessionId && controls.state.title
            ? { ...session, title: controls.state.title }
            : session,
        ),
      })),
    [controls.state.title, liveSessionId, workspaces],
  );

  const startNewSession = useCallback(() => {
    nextSessionInstance.current += 1;
    setSelection({
      kind: 'new',
      instanceId: nextSessionInstance.current,
      workspace: controls.state.workspace ?? undefined,
    });
    setSidebarOpen(false);
  }, [controls.state.workspace]);

  const handleSelectWorkspace = useCallback((workspace: string) => {
    nextSessionInstance.current += 1;
    setSelection((current) =>
      current.kind === 'new'
        ? {
            kind: 'new',
            instanceId: nextSessionInstance.current,
            workspace,
          }
        : current,
    );
  }, []);

  const handleSelect = useCallback(
    (sessionId: string) => {
      const alreadySelected =
        (selection.kind === 'session' && selection.sessionId === sessionId) ||
        liveSessionId === sessionId;
      setSidebarOpen(false);
      if (alreadySelected) {
        if (controls.connectionPhase === 'error') controls.retry();
        return;
      }
      setSelection({ kind: 'session', sessionId });
    },
    [controls.connectionPhase, controls.retry, liveSessionId, selection],
  );

  const openScheduledTasks = useCallback(() => {
    setSelection({ kind: 'scheduled_tasks' });
    setSidebarOpen(false);
  }, []);

  const handleSetPinned = useCallback(
    (sessionId: string, pinned: boolean) => setSessionPinned(sessionId, pinned),
    [setSessionPinned],
  );

  const handleDeleteSession = useCallback(
    async (sessionId: string) => {
      await deleteSession(sessionId);
      const deletedActiveSession =
        (selection.kind === 'session' && selection.sessionId === sessionId) ||
        liveSessionId === sessionId;
      if (deletedActiveSession) startNewSession();
    },
    [deleteSession, liveSessionId, selection, startNewSession],
  );

  const handleFork = useCallback(
    async (messageIndex: number, position: ForkPosition = 'after') => {
      const sessionId = controls.state.sessionId;
      if (sessionId === null) {
        throw new Error('The current session has not been activated yet.');
      }
      const forked = await forkSession(
        config.authKey,
        sessionId,
        messageIndex,
        position,
      );
      await refreshSessions();
      setSelection({ kind: 'session', sessionId: forked.session_id });
      setSidebarOpen(false);
    },
    [config.authKey, controls.state.sessionId, refreshSessions],
  );

  const handleSaveAuthKey = useCallback((key: string) => {
    writeAuthKey(key);
    setConfig((current) => ({ ...current, authKey: key }));
  }, []);

  const handleSaveProfileId = useCallback((id: string) => {
    writeProfileId(id);
    setConfig((current) => ({ ...current, profileId: id }));
  }, []);

  const handleProviderSaved = useCallback((saved: PublicProviderConfig) => {
    setProviderProfiles((current) => {
      const index = current.findIndex(
        (profile) => profile.profile_id === saved.profile_id,
      );
      if (index < 0) return [...current, saved];
      return current.map((profile, profileIndex) =>
        profileIndex === index ? saved : profile,
      );
    });
  }, []);

  const handleSelectProvider = useCallback(
    (profileId: string) => {
      const nextProfileId = profileId.trim();
      if (!nextProfileId) return;
      writeProfileId(nextProfileId);
      setConfig((current) => ({ ...current, profileId: nextProfileId }));
      nextSessionInstance.current += 1;
      setSelection({
        kind: 'new',
        instanceId: nextSessionInstance.current,
        workspace: controls.state.workspace ?? undefined,
      });
      setSidebarOpen(false);
    },
    [controls.state.workspace],
  );

  const handleSaveAgentProfileId = useCallback((id: string) => {
    const value = id.trim();
    writeAgentProfileId(value);
    setConfig((current) => ({ ...current, agentProfileId: value }));
  }, []);

  const handleSaveCapabilityMode = useCallback(
    (capabilityMode: CapabilityMode | null) => {
      writeCapabilityMode(capabilityMode);
      setConfig((current) => ({ ...current, capabilityMode }));
    },
    [],
  );

  const handleConfigured = useCallback(() => {
    setSelection((current) => {
      if (current.kind !== 'none') return current;
      nextSessionInstance.current += 1;
      return {
        kind: 'new',
        instanceId: nextSessionInstance.current,
      };
    });
  }, []);

  const cycleLocale = useCallback(() => {
    const currentIndex = LOCALES.indexOf(locale);
    const next = LOCALES[(currentIndex + 1) % LOCALES.length];
    if (next) setLocale(next);
  }, [locale, setLocale]);

  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
  }, [locale]);

  const activeSessionId =
    selection.kind === 'scheduled_tasks'
      ? null
      : selection.kind === 'session'
        ? selection.sessionId
        : liveSessionId;
  const conversationKey =
    selection.kind === 'new'
      ? `new:${selection.instanceId}`
      : selection.kind === 'session'
        ? `session:${selection.sessionId}`
        : 'none';

  return (
    <div className={styles.app}>
      <Sidebar
        open={sidebarOpen}
        workspaces={visibleWorkspaces}
        loading={sessionsLoading}
        activeSessionId={activeSessionId}
        listError={listError}
        profileId={config.profileId}
        theme={theme}
        onSelect={handleSelect}
        onSetPinned={handleSetPinned}
        onDelete={handleDeleteSession}
        onNewChat={startNewSession}
        scheduledTasksActive={selection.kind === 'scheduled_tasks'}
        onOpenScheduledTasks={openScheduledTasks}
        onOpenSettings={() => setSettingsOpen(true)}
        onToggleTheme={toggleTheme}
        onCycleLocale={cycleLocale}
        onClose={() => setSidebarOpen(false)}
      />

      <main className={styles.main}>
        {selection.kind === 'scheduled_tasks' ? (
          <ScheduledTasksPage
            authKey={config.authKey}
            profileId={config.profileId}
            agentProfileId={config.agentProfileId}
            capabilityMode={config.capabilityMode}
            onOpenSession={handleSelect}
            onSessionsChanged={refreshSessions}
            onOpenSidebar={() => setSidebarOpen(true)}
          />
        ) : (
          <Chat
            controls={controls}
            authKey={config.authKey}
            profileId={config.profileId}
            providerProfiles={providerProfiles}
            conversationKey={conversationKey}
            onFork={handleFork}
            onSelectProvider={handleSelectProvider}
            onSelectWorkspace={handleSelectWorkspace}
            onOpenSidebar={() => setSidebarOpen(true)}
            onOpenSettings={() => setSettingsOpen(true)}
          />
        )}
      </main>

      {settingsOpen && (
        <SettingsModal
          authKey={config.authKey}
          profileId={config.profileId}
          agentProfileId={config.agentProfileId}
          capabilityMode={config.capabilityMode}
          onClose={() => setSettingsOpen(false)}
          onSaveAuthKey={handleSaveAuthKey}
          onSaveProfileId={handleSaveProfileId}
          onSaveAgentProfileId={handleSaveAgentProfileId}
          onSaveCapabilityMode={handleSaveCapabilityMode}
          onProviderSaved={handleProviderSaved}
          onConfigured={handleConfigured}
        />
      )}
    </div>
  );
}

export default App;
