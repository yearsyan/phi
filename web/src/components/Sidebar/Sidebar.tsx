import {
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useRef,
  useState,
} from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import { LOCALE_LABELS, LOCALES } from '../../i18n/translations.ts';
import type { Theme } from '../../prefs.ts';
import type {
  SessionStatus,
  SessionSummary,
  WorkspaceSessionGroup,
} from '../../types/wire.ts';
import {
  ChevronIcon,
  ClockIcon,
  CloseIcon,
  FolderIcon,
  GearIcon,
  GlobeIcon,
  MoonIcon,
  PinIcon,
  PlusIcon,
  SunIcon,
  TrashIcon,
} from '../common/Icons.tsx';
import styles from './Sidebar.module.css';

interface SidebarProps {
  open: boolean;
  workspaces: WorkspaceSessionGroup[];
  loading: boolean;
  activeSessionId: string | null;
  listError: string | null;
  profileId: string;
  theme: Theme;
  onSelect: (sessionId: string) => void;
  onSetPinned: (sessionId: string, pinned: boolean) => Promise<void>;
  onDelete: (sessionId: string) => Promise<void>;
  onNewChat: () => void;
  scheduledTasksActive?: boolean;
  onOpenScheduledTasks?: () => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
  onCycleLocale: () => void;
  onClose: () => void;
}

export function Sidebar({
  open,
  workspaces,
  loading,
  activeSessionId,
  listError,
  profileId,
  theme,
  onSelect,
  onSetPinned,
  onDelete,
  onNewChat,
  scheduledTasksActive = false,
  onOpenScheduledTasks,
  onOpenSettings,
  onToggleTheme,
  onCycleLocale,
  onClose,
}: SidebarProps) {
  const { t, locale } = useI18n();
  const [contextMenu, setContextMenu] = useState<SessionContextMenu | null>(
    null,
  );
  const [pendingSessionId, setPendingSessionId] = useState<string | null>(null);
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<Set<string>>(
    () => new Set(),
  );
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const contextSession = contextMenu
    ? findSession(workspaces, contextMenu.sessionId)
    : undefined;
  const hasSessions = workspaces.some((group) => group.sessions.length > 0);

  useEffect(() => {
    if (!contextMenu) return;
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!contextMenuRef.current?.contains(event.target as Node)) {
        setContextMenu(null);
      }
    };
    const closeOnKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setContextMenu(null);
    };
    const close = () => setContextMenu(null);
    document.addEventListener('pointerdown', closeOnPointerDown);
    document.addEventListener('keydown', closeOnKeyDown);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    contextMenuRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitem"]')
      ?.focus();
    return () => {
      document.removeEventListener('pointerdown', closeOnPointerDown);
      document.removeEventListener('keydown', closeOnKeyDown);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [contextMenu]);

  const showContextMenu = (
    sessionId: string,
    clientX: number,
    clientY: number,
  ) => {
    const menuWidth = 184;
    const menuHeight = 92;
    setContextMenu({
      sessionId,
      left: Math.max(8, Math.min(clientX, window.innerWidth - menuWidth - 8)),
      top: Math.max(8, Math.min(clientY, window.innerHeight - menuHeight - 8)),
    });
  };

  const handleContextMenu = (
    event: ReactMouseEvent<HTMLButtonElement>,
    sessionId: string,
  ) => {
    event.preventDefault();
    showContextMenu(sessionId, event.clientX, event.clientY);
  };

  const handleSessionKeyDown = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    sessionId: string,
  ) => {
    if (
      event.key !== 'ContextMenu' &&
      !(event.shiftKey && event.key === 'F10')
    ) {
      return;
    }
    event.preventDefault();
    const bounds = event.currentTarget.getBoundingClientRect();
    showContextMenu(sessionId, bounds.left + 24, bounds.top + 24);
  };

  const togglePinned = async (session: SessionSummary) => {
    setContextMenu(null);
    setPendingSessionId(session.session_id);
    try {
      await onSetPinned(session.session_id, !session.pinned);
    } catch {
      // The list-level error supplied by the hook exposes the failure.
    } finally {
      setPendingSessionId((current) =>
        current === session.session_id ? null : current,
      );
    }
  };

  const deleteSelectedSession = async (session: SessionSummary) => {
    setContextMenu(null);
    const title = session.title ?? sessionTitle(session.session_id);
    if (!window.confirm(t('sidebar.deleteConfirm', { title }))) return;
    setPendingSessionId(session.session_id);
    try {
      await onDelete(session.session_id);
    } catch {
      // The list-level error supplied by the hook exposes the failure.
    } finally {
      setPendingSessionId((current) =>
        current === session.session_id ? null : current,
      );
    }
  };

  const toggleWorkspace = (workspaceKey: string) => {
    setCollapsedWorkspaces((current) => {
      const next = new Set(current);
      if (next.has(workspaceKey)) next.delete(workspaceKey);
      else next.add(workspaceKey);
      return next;
    });
  };

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

        <button
          type="button"
          className={`${styles.scheduledButton} ${scheduledTasksActive ? styles.scheduledButtonActive : ''}`}
          onClick={onOpenScheduledTasks}
          aria-current={scheduledTasksActive ? 'page' : undefined}
        >
          <ClockIcon />
          <span>{t('sidebar.scheduledTasks')}</span>
        </button>

        <div className={styles.sectionHeader}>
          <span>{t('sidebar.workspaces')}</span>
        </div>

        <nav className={styles.sessionList}>
          {listError && (
            <div className={styles.listError} role="status">
              {listError}
            </div>
          )}

          {!hasSessions && !loading && !listError && (
            <div className={styles.empty}>
              <span>{t('sidebar.noSessions')}</span>
              <small>{t('sidebar.noSessionsHint')}</small>
            </div>
          )}

          {workspaces.map((group, groupIndex) => {
            const groupKey = workspaceKey(group.workspace);
            const collapsed = collapsedWorkspaces.has(groupKey);
            const label = workspaceLabel(
              group.workspace,
              t('sidebar.unassignedWorkspace'),
            );
            const childrenId = `workspace-sessions-${groupIndex}`;
            return (
              <div className={styles.workspaceGroup} key={groupKey}>
                <button
                  type="button"
                  className={styles.workspaceNode}
                  title={group.workspace ?? t('sidebar.unassignedWorkspace')}
                  aria-expanded={!collapsed}
                  aria-controls={childrenId}
                  aria-label={t('sidebar.workspaceGroup', {
                    workspace: label,
                    count: group.sessions.length,
                  })}
                  onClick={() => toggleWorkspace(groupKey)}
                >
                  <ChevronIcon
                    className={`${styles.workspaceChevron} ${collapsed ? styles.workspaceChevronCollapsed : ''}`}
                  />
                  <span className={styles.workspaceIcon} aria-hidden="true">
                    <FolderIcon />
                    <GlobeIcon />
                  </span>
                  <span className={styles.workspaceName}>{label}</span>
                  <span className={styles.workspaceCount}>
                    {group.sessions.length}
                  </span>
                  <span
                    className={`${styles.statusDot} ${styles.workspaceStatus} ${statusClass(workspaceStatus(group.sessions), styles)}`}
                    aria-hidden="true"
                  />
                </button>

                {!collapsed && (
                  <fieldset
                    id={childrenId}
                    className={styles.workspaceSessions}
                  >
                    <legend className={styles.workspaceLegend}>{label}</legend>
                    {group.sessions.map((session) => {
                      const active = session.session_id === activeSessionId;
                      const pending = session.session_id === pendingSessionId;
                      return (
                        <button
                          type="button"
                          key={session.session_id}
                          className={`${styles.session} ${active ? styles.sessionActive : ''} ${pending ? styles.sessionPending : ''}`}
                          onClick={() => onSelect(session.session_id)}
                          onContextMenu={(event) =>
                            handleContextMenu(event, session.session_id)
                          }
                          onKeyDown={(event) =>
                            handleSessionKeyDown(event, session.session_id)
                          }
                          aria-busy={pending || undefined}
                          disabled={pending}
                        >
                          <div className={styles.sessionTop}>
                            <span
                              className={`${styles.statusDot} ${statusClass(session.status, styles)}`}
                            />
                            <span className={styles.sessionTitle}>
                              {session.title ??
                                sessionTitle(session.session_id)}
                            </span>
                            {session.pinned && (
                              <span
                                className={styles.pinMarker}
                                title={t('sidebar.pinned')}
                                role="img"
                                aria-label={t('sidebar.pinned')}
                              >
                                <PinIcon />
                              </span>
                            )}
                            <span className={styles.messageCount}>
                              {session.message_count ?? '—'}
                            </span>
                          </div>
                          <div className={styles.sessionMeta}>
                            <span>{session.config.model}</span>
                          </div>
                        </button>
                      );
                    })}
                  </fieldset>
                )}
              </div>
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

      {contextMenu && contextSession && (
        <div
          ref={contextMenuRef}
          className={styles.contextMenu}
          role="menu"
          aria-label={t('sidebar.sessionActions')}
          style={{ left: contextMenu.left, top: contextMenu.top }}
        >
          <button
            type="button"
            className={styles.contextMenuItem}
            role="menuitem"
            onClick={() => void togglePinned(contextSession)}
          >
            <PinIcon />
            <span>
              {contextSession.pinned ? t('sidebar.unpin') : t('sidebar.pin')}
            </span>
          </button>
          <button
            type="button"
            className={`${styles.contextMenuItem} ${styles.contextMenuDanger}`}
            role="menuitem"
            onClick={() => void deleteSelectedSession(contextSession)}
          >
            <TrashIcon />
            <span>{t('sidebar.delete')}</span>
          </button>
        </div>
      )}
    </>
  );
}

interface SessionContextMenu {
  sessionId: string;
  left: number;
  top: number;
}

function findSession(
  workspaces: readonly WorkspaceSessionGroup[],
  sessionId: string,
): SessionSummary | undefined {
  for (const workspace of workspaces) {
    const session = workspace.sessions.find(
      (candidate) => candidate.session_id === sessionId,
    );
    if (session) return session;
  }
  return undefined;
}

function workspaceKey(workspace: string | null): string {
  return workspace ?? '__phi_unassigned_workspace__';
}

function workspaceLabel(workspace: string | null, fallback: string): string {
  if (!workspace) return fallback;
  const normalized = workspace.replace(/[\\/]+$/, '');
  if (normalized.length === 0) return workspace.startsWith('\\') ? '\\' : '/';
  return normalized.split(/[\\/]/).pop() ?? normalized;
}

function workspaceStatus(sessions: readonly SessionSummary[]): SessionStatus {
  if (
    sessions.some(
      (session) =>
        session.status === 'running' || session.status === 'compacting',
    )
  ) {
    return 'running';
  }
  if (
    sessions.some(
      (session) =>
        session.status === 'stopping' || session.status === 'closing',
    )
  ) {
    return 'stopping';
  }
  if (
    sessions.some(
      (session) => session.status !== 'closed' && session.status !== 'offline',
    )
  ) {
    return 'idle';
  }
  return 'offline';
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
