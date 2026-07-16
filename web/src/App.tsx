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
import { Chat } from './components/Chat/Chat.tsx';
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
import type { CapabilityMode } from './types/wire.ts';

type Selection =
  | { kind: 'none' }
  | { kind: 'new'; instanceId: number }
  | { kind: 'session'; sessionId: string };

function selectionToTarget(
  selection: Selection,
  profileId: string,
  agentProfileId: string,
  capabilityMode: CapabilityMode | null,
): SessionTarget | null {
  if (selection.kind === 'none') return null;
  if (selection.kind === 'new') {
    return {
      kind: 'new',
      profileId,
      agentProfileId: agentProfileId.trim() || undefined,
      capabilityMode: capabilityMode ?? undefined,
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

  useEffect(() => {
    if (!isConfigured(config)) setSettingsOpen(true);
  }, [config]);

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
    sessions,
    loading: sessionsLoading,
    error: listError,
    refresh: refreshSessions,
  } = useSessionList(config.authKey, isConfigured(config));

  const { createdSessionRevision } = controls;
  useEffect(() => {
    if (createdSessionRevision > 0) void refreshSessions();
  }, [createdSessionRevision, refreshSessions]);

  const liveSessionId = controls.state.sessionId;

  const startNewSession = useCallback(() => {
    nextSessionInstance.current += 1;
    setSelection({
      kind: 'new',
      instanceId: nextSessionInstance.current,
    });
    setSidebarOpen(false);
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

  const handleSaveAuthKey = useCallback((key: string) => {
    writeAuthKey(key);
    setConfig((current) => ({ ...current, authKey: key }));
  }, []);

  const handleSaveProfileId = useCallback((id: string) => {
    writeProfileId(id);
    setConfig((current) => ({ ...current, profileId: id }));
  }, []);

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
    setSettingsOpen(false);
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
    selection.kind === 'session' ? selection.sessionId : liveSessionId;
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
        sessions={sessions}
        loading={sessionsLoading}
        activeSessionId={activeSessionId}
        listError={listError}
        profileId={config.profileId}
        theme={theme}
        onSelect={handleSelect}
        onNewChat={startNewSession}
        onOpenSettings={() => setSettingsOpen(true)}
        onToggleTheme={toggleTheme}
        onCycleLocale={cycleLocale}
        onClose={() => setSidebarOpen(false)}
      />

      <main className={styles.main}>
        <Chat
          controls={controls}
          profileId={config.profileId}
          conversationKey={conversationKey}
          onOpenSidebar={() => setSidebarOpen(true)}
          onOpenSettings={() => setSettingsOpen(true)}
        />
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
          onConfigured={handleConfigured}
        />
      )}
    </div>
  );
}

export default App;
