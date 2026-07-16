import type { ConnectionPhase } from '../../hooks/useDaemonSession.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import {
  LOCALE_LABELS,
  LOCALES,
  type TranslationKey,
} from '../../i18n/translations.ts';
import type { Theme } from '../../prefs.ts';
import type { SessionSummary } from '../../types/wire.ts';
import {
  GearIcon,
  GlobeIcon,
  MoonIcon,
  PlusIcon,
  SunIcon,
} from '../common/Icons.tsx';
import styles from './Sidebar.module.css';

interface SidebarProps {
  sessions: SessionSummary[];
  activeSessionId: string | null;
  newSessionPhase: ConnectionPhase | null;
  listError: string | null;
  profileId: string;
  theme: Theme;
  onSelect: (sessionId: string) => void;
  onNewChat: () => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
  onCycleLocale: () => void;
}

export function Sidebar({
  sessions,
  activeSessionId,
  newSessionPhase,
  listError,
  profileId,
  theme,
  onSelect,
  onNewChat,
  onOpenSettings,
  onToggleTheme,
  onCycleLocale,
}: SidebarProps) {
  const { t, locale } = useI18n();

  return (
    <aside className={styles.sidebar}>
      <div className={styles.topRow}>
        <div className={styles.brand}>
          <span className={styles.logo}>φ</span>
          <span className={styles.brandText}>Phi</span>
        </div>
        <div className={styles.tools}>
          <button
            type="button"
            className={styles.iconBtn}
            onClick={onCycleLocale}
            title={LOCALES.map((code) => LOCALE_LABELS[code]).join(' / ')}
            aria-label={t('lang.toggle')}
          >
            <GlobeIcon />
            <span className={styles.localeCode}>{locale}</span>
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            onClick={onToggleTheme}
            title={theme === 'dark' ? t('theme.light') : t('theme.dark')}
            aria-label={t('theme.toggle')}
          >
            {theme === 'dark' ? <SunIcon /> : <MoonIcon />}
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            onClick={onOpenSettings}
            title={t('sidebar.settings')}
            aria-label={t('sidebar.settings')}
          >
            <GearIcon />
          </button>
        </div>
      </div>

      <button type="button" className={styles.newBtn} onClick={onNewChat}>
        <PlusIcon /> {t('sidebar.newChat')}
      </button>

      <div className={styles.listWrap}>
        {listError && <div className={styles.listError}>{listError}</div>}
        {newSessionPhase !== null && (
          <div
            className={`${styles.item} ${styles.itemActive} ${styles.itemPrepared}`}
          >
            <span className={styles.itemTitle}>{t('sidebar.newSession')}</span>
            <span className={styles.itemMeta}>
              {t(NEW_SESSION_PHASE_KEYS[newSessionPhase])}
            </span>
          </div>
        )}
        {sessions.length === 0 && newSessionPhase === null && !listError && (
          <div className={styles.emptyHint}>{t('sidebar.noSessions')}</div>
        )}
        {sessions.map((session) => {
          const active = session.session_id === activeSessionId;
          const count = session.message_count ?? 0;
          return (
            <button
              type="button"
              key={session.session_id}
              className={`${styles.item} ${active ? styles.itemActive : ''}`}
              onClick={() => onSelect(session.session_id)}
            >
              <div className={styles.itemHead}>
                <span
                  className={`${styles.itemDot} ${styles[`status_${session.status}`] ?? ''}`}
                />
                <span className={styles.itemTitle}>
                  {sessionTitle(session)}
                </span>
              </div>
              <div className={styles.itemMeta}>
                <span>{session.config.model}</span>
                {session.message_count !== null && (
                  <span>
                    {' · '}
                    {count}{' '}
                    {count === 1 ? t('sidebar.msg') : t('sidebar.messages')}
                  </span>
                )}
              </div>
            </button>
          );
        })}
      </div>

      <div className={styles.footer}>
        <span className={styles.footerLabel}>{t('sidebar.profile')}</span>
        <span className={styles.footerValue}>{profileId}</span>
      </div>
    </aside>
  );
}

const NEW_SESSION_PHASE_KEYS = {
  idle: 'chat.connection.idle',
  connecting: 'chat.connection.connecting',
  preparing: 'sidebar.preparing',
  ready: 'chat.status.awaiting_first_prompt',
  error: 'chat.connection.error',
} satisfies Record<ConnectionPhase, TranslationKey>;

function sessionTitle(session: SessionSummary): string {
  const id = session.session_id;
  return id.length > 8 ? `…${id.slice(-6)}` : id;
}
