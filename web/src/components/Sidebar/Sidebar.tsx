import { useI18n } from '../../i18n/I18nProvider.tsx';
import { LOCALE_LABELS, LOCALES } from '../../i18n/translations.ts';
import type { Theme } from '../../prefs.ts';
import type { SessionStatus, SessionSummary } from '../../types/wire.ts';
import {
  CloseIcon,
  GearIcon,
  GlobeIcon,
  MoonIcon,
  PlusIcon,
  SunIcon,
} from '../common/Icons.tsx';
import styles from './Sidebar.module.css';

interface SidebarProps {
  open: boolean;
  sessions: SessionSummary[];
  loading: boolean;
  activeSessionId: string | null;
  listError: string | null;
  profileId: string;
  theme: Theme;
  onSelect: (sessionId: string) => void;
  onNewChat: () => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
  onCycleLocale: () => void;
  onClose: () => void;
}

export function Sidebar({
  open,
  sessions,
  loading,
  activeSessionId,
  listError,
  profileId,
  theme,
  onSelect,
  onNewChat,
  onOpenSettings,
  onToggleTheme,
  onCycleLocale,
  onClose,
}: SidebarProps) {
  const { t, locale } = useI18n();

  return (
    <>
      <button
        type="button"
        className={`${styles.backdrop} ${open ? styles.backdropVisible : ''}`}
        aria-label={t('sidebar.close')}
        onClick={onClose}
      />
      <aside
        className={`${styles.sidebar} ${open ? styles.sidebarOpen : ''}`}
        aria-label={t('sidebar.sessions')}
      >
        <button
          type="button"
          className={`${styles.iconButton} ${styles.mobileClose}`}
          onClick={onClose}
          aria-label={t('sidebar.close')}
        >
          <CloseIcon />
        </button>

        <button type="button" className={styles.newButton} onClick={onNewChat}>
          <span className={styles.newIcon}>
            <PlusIcon />
          </span>
          <span>
            <strong>{t('sidebar.newChat')}</strong>
            <small>{t('sidebar.newChatHint')}</small>
          </span>
        </button>

        <div className={styles.sectionHeader}>
          <span>{t('sidebar.recent')}</span>
        </div>

        <nav className={styles.sessionList}>
          {listError && (
            <div className={styles.listError} role="status">
              {listError}
            </div>
          )}

          {sessions.length === 0 && !loading && !listError && (
            <div className={styles.empty}>
              <span>{t('sidebar.noSessions')}</span>
              <small>{t('sidebar.noSessionsHint')}</small>
            </div>
          )}

          {sessions.map((session) => {
            const active = session.session_id === activeSessionId;
            return (
              <button
                type="button"
                key={session.session_id}
                className={`${styles.session} ${active ? styles.sessionActive : ''}`}
                onClick={() => onSelect(session.session_id)}
              >
                <div className={styles.sessionTop}>
                  <span
                    className={`${styles.statusDot} ${statusClass(session.status, styles)}`}
                  />
                  <span className={styles.sessionTitle}>
                    {sessionTitle(session.session_id)}
                  </span>
                  <span className={styles.messageCount}>
                    {session.message_count ?? '—'}
                  </span>
                </div>
                <div className={styles.sessionMeta}>
                  <span>{session.config.model}</span>
                  <span>{session.mode ?? 'default'}</span>
                </div>
              </button>
            );
          })}
        </nav>

        <footer className={styles.footer}>
          <button
            type="button"
            className={styles.profileButton}
            onClick={onOpenSettings}
          >
            <span className={styles.profileMark}>{profileId.slice(0, 1)}</span>
            <span className={styles.profileText}>
              <small>{t('sidebar.profile')}</small>
              <strong>{profileId}</strong>
            </span>
            <GearIcon />
          </button>

          <div className={styles.footerActions}>
            <button
              type="button"
              className={styles.utilityButton}
              onClick={onCycleLocale}
              title={LOCALES.map((code) => LOCALE_LABELS[code]).join(' / ')}
              aria-label={t('lang.toggle')}
            >
              <GlobeIcon />
              <span>{locale.toUpperCase()}</span>
            </button>
            <button
              type="button"
              className={styles.utilityButton}
              onClick={onToggleTheme}
              aria-label={t('theme.toggle')}
            >
              {theme === 'dark' ? <SunIcon /> : <MoonIcon />}
              <span>
                {theme === 'dark' ? t('theme.light') : t('theme.dark')}
              </span>
            </button>
          </div>
        </footer>
      </aside>
    </>
  );
}

function sessionTitle(sessionId: string): string {
  return sessionId.length > 12
    ? `Session ${sessionId.slice(-6)}`
    : `Session ${sessionId}`;
}

function statusClass(
  status: SessionStatus,
  classes: Record<string, string>,
): string {
  if (status === 'running' || status === 'compacting') {
    return classes.statusBusy ?? '';
  }
  if (status === 'stopping' || status === 'closing') {
    return classes.statusWarning ?? '';
  }
  if (status === 'closed' || status === 'offline') {
    return classes.statusOffline ?? '';
  }
  return classes.statusReady ?? '';
}
