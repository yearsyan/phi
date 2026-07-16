import { useCallback, useEffect, useMemo, useState } from 'react';
import styles from './App.module.css';
import {
  isConfigured,
  readDaemonConfig,
  writeAuthKey,
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

type Selection =
  | { kind: 'none' }
  | { kind: 'new' }
  | { kind: 'session'; sessionId: string };

function selectionToTarget(
  selection: Selection,
  profileId: string,
): SessionTarget | null {
  if (selection.kind === 'none') return null;
  if (selection.kind === 'new') return { kind: 'new', profileId };
  return { kind: 'attach', sessionId: selection.sessionId };
}

function App() {
  // Read persisted prefs once for the initial render so the provider and theme
  // hook start in sync with storage.
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

  const [config, setConfig] = useState(() => readDaemonConfig());
  const [selection, setSelection] = useState<Selection>(() =>
    isConfigured(config) ? { kind: 'new' } : { kind: 'none' },
  );
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Open settings automatically on first load if no auth key is configured.
  // biome-ignore lint/correctness/useExhaustiveDependencies: mount-only check
  useEffect(() => {
    if (!isConfigured(config)) {
      setSettingsOpen(true);
    }
  }, []);

  // Memoize the target so its identity is stable across renders that don't
  // change the selected session/profile — otherwise useDaemonSession's effect
  // would tear down and reopen the WebSocket on every render.
  const target = useMemo(
    () => selectionToTarget(selection, config.profileId),
    [selection, config.profileId],
  );
  const controls = useDaemonSession(config.authKey, target);

  // Track the live session id so the sidebar can highlight the prepared session
  // once it activates (after `session_created`).
  const liveSessionId = controls.state.sessionId;

  // Once a "new" target activates (session_created), flip the selection to that
  // session id so reconnects/future targeting use attach semantics.
  useEffect(() => {
    if (selection.kind === 'new' && liveSessionId) {
      setSelection({ kind: 'session', sessionId: liveSessionId });
    }
  }, [selection.kind, liveSessionId]);

  const { sessions, error: listError } = useSessionList(
    config.authKey,
    isConfigured(config),
  );

  const handleNewChat = useCallback(() => {
    setSelection({ kind: 'new' });
  }, []);

  const handleSelect = useCallback(
    (sessionId: string) => {
      const alreadySelected =
        (selection.kind === 'session' && selection.sessionId === sessionId) ||
        liveSessionId === sessionId;
      if (alreadySelected) {
        if (controls.connectionPhase === 'error') {
          controls.retry();
        }
        return;
      }
      setSelection({ kind: 'session', sessionId });
    },
    [controls.connectionPhase, controls.retry, liveSessionId, selection],
  );

  const handleSaveAuthKey = useCallback((key: string) => {
    writeAuthKey(key);
    setConfig((prev) => ({ ...prev, authKey: key }));
  }, []);

  const handleSaveProfileId = useCallback((id: string) => {
    writeProfileId(id);
    setConfig((prev) => ({ ...prev, profileId: id }));
  }, []);

  const handleConfigured = useCallback(() => {
    setSelection((current) =>
      current.kind === 'none' ? { kind: 'new' } : current,
    );
  }, []);

  // Cycle en → zh → en (and any future locales) on each click.
  const cycleLocale = useCallback(() => {
    const currentIndex = LOCALES.indexOf(locale);
    const next = LOCALES[(currentIndex + 1) % LOCALES.length];
    if (next) {
      setLocale(next);
      writeLocale(next);
    }
  }, [locale, setLocale]);

  // Keep <html lang> in sync with the active locale for a11y.
  useEffect(() => {
    document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
  }, [locale]);

  const activeSessionId =
    selection.kind === 'session' ? selection.sessionId : liveSessionId;
  const newSessionPhase =
    selection.kind === 'new' && !liveSessionId
      ? controls.connectionPhase
      : null;

  return (
    <div className={styles.app}>
      <Sidebar
        sessions={sessions}
        activeSessionId={activeSessionId}
        newSessionPhase={newSessionPhase}
        listError={listError}
        onSelect={handleSelect}
        onNewChat={handleNewChat}
        onOpenSettings={() => setSettingsOpen(true)}
        onToggleTheme={toggleTheme}
        onCycleLocale={cycleLocale}
        theme={theme}
        profileId={config.profileId}
      />
      <main className={styles.main}>
        <Chat controls={controls} />
      </main>

      {settingsOpen && (
        <SettingsModal
          authKey={config.authKey}
          profileId={config.profileId}
          onClose={() => setSettingsOpen(false)}
          onSaveAuthKey={handleSaveAuthKey}
          onSaveProfileId={handleSaveProfileId}
          onConfigured={handleConfigured}
        />
      )}
    </div>
  );
}

export default App;
