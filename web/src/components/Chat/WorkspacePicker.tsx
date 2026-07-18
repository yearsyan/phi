import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { browseWorkspace } from '../../api/http.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { WorkspaceBrowseResponse } from '../../types/wire.ts';
import {
  ArrowUpIcon,
  ChevronIcon,
  CloseIcon,
  FolderIcon,
  PlusIcon,
} from '../common/Icons.tsx';
import styles from './WorkspacePicker.module.css';

interface WorkspacePickerProps {
  authKey: string;
  workspace: string | null;
  disabled?: boolean;
  onSelect: (workspace: string) => void;
}

const RECENT_WORKSPACES_KEY = 'phi.prefs.recentWorkspaces';
const MAX_RECENT_WORKSPACES = 12;

export function workspaceName(workspace: string): string {
  const normalized = workspace.replace(/[\\/]+$/, '');
  if (normalized.length === 0) return workspace.startsWith('\\') ? '\\' : '/';
  return normalized.split(/[\\/]/).pop() ?? normalized;
}

export function WorkspacePicker({
  authKey,
  workspace,
  disabled = false,
  onSelect,
}: WorkspacePickerProps) {
  const { t } = useI18n();
  const rootRef = useRef<HTMLDivElement | null>(null);
  const pathInputRef = useRef<HTMLInputElement | null>(null);
  const requestRef = useRef(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const [browserOpen, setBrowserOpen] = useState(false);
  const [recentWorkspaces, setRecentWorkspaces] = useState(() =>
    rememberWorkspace(readRecentWorkspaces(), workspace),
  );
  const [listing, setListing] = useState<WorkspaceBrowseResponse | null>(null);
  const [pathDraft, setPathDraft] = useState(workspace ?? '');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    async (path?: string) => {
      const request = requestRef.current + 1;
      requestRef.current = request;
      setLoading(true);
      setError(null);
      try {
        const next = await browseWorkspace(authKey, path);
        if (requestRef.current !== request) return;
        setListing(next);
        setPathDraft(next.path);
      } catch (loadError) {
        if (requestRef.current !== request) return;
        setError(
          loadError instanceof Error ? loadError.message : String(loadError),
        );
      } finally {
        if (requestRef.current === request) setLoading(false);
      }
    },
    [authKey],
  );

  const closeBrowser = useCallback(() => {
    requestRef.current += 1;
    setBrowserOpen(false);
    setLoading(false);
    setError(null);
  }, []);

  useEffect(() => {
    if (!workspace) return;
    setRecentWorkspaces((current) => {
      const next = rememberWorkspace(current, workspace);
      writeRecentWorkspaces(next);
      return next;
    });
  }, [workspace]);

  useEffect(() => {
    if (!browserOpen) return;
    void load(workspace ?? undefined);
    pathInputRef.current?.focus();
  }, [browserOpen, load, workspace]);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [menuOpen]);

  useEffect(() => {
    if (!browserOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') closeBrowser();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [browserOpen, closeBrowser]);

  const selectWorkspace = (nextWorkspace: string) => {
    const next = rememberWorkspace(recentWorkspaces, nextWorkspace);
    setRecentWorkspaces(next);
    writeRecentWorkspaces(next);
    setMenuOpen(false);
    if (nextWorkspace !== workspace) onSelect(nextWorkspace);
  };

  const openBrowser = () => {
    setMenuOpen(false);
    setBrowserOpen(true);
  };

  return (
    <div className={styles.picker} ref={rootRef}>
      <button
        type="button"
        className={styles.trigger}
        disabled={disabled || workspace === null}
        onClick={() => setMenuOpen((current) => !current)}
        aria-expanded={menuOpen}
        aria-haspopup="menu"
        title={workspace ?? t('chat.workspace.choose')}
      >
        <FolderIcon />
        <span>
          {workspace ? workspaceName(workspace) : t('chat.workspace.loading')}
        </span>
        <ChevronIcon />
      </button>

      {menuOpen && (
        <div
          className={styles.workspaceMenu}
          role="menu"
          aria-label={t('chat.workspace.recent')}
        >
          <div
            className={styles.workspaceList}
            data-testid="workspace-scroll-list"
          >
            {recentWorkspaces.map((recentWorkspace) => {
              const selected = recentWorkspace === workspace;
              return (
                <button
                  type="button"
                  className={`${styles.workspaceOption} ${selected ? styles.workspaceOptionSelected : ''}`}
                  key={recentWorkspace}
                  role="menuitemradio"
                  aria-checked={selected}
                  title={recentWorkspace}
                  onClick={() => selectWorkspace(recentWorkspace)}
                >
                  <FolderIcon />
                  <span>
                    <strong>{workspaceName(recentWorkspace)}</strong>
                    <small>{recentWorkspace}</small>
                  </span>
                </button>
              );
            })}
          </div>
          <div className={styles.workspaceMenuFooter}>
            <button
              type="button"
              className={styles.addWorkspace}
              role="menuitem"
              onClick={openBrowser}
            >
              <PlusIcon />
              <span>{t('chat.workspace.add')}</span>
            </button>
          </div>
        </div>
      )}

      {browserOpen &&
        createPortal(
          <div className={styles.modalBackdrop}>
            <div
              className={styles.browserModal}
              role="dialog"
              aria-modal="true"
              aria-label={t('chat.workspace.add')}
            >
              <div className={styles.header}>
                <div>
                  <strong>{t('chat.workspace.add')}</strong>
                  <span>{t('chat.workspace.browserHint')}</span>
                </div>
                <button
                  type="button"
                  className={styles.closeButton}
                  onClick={closeBrowser}
                  aria-label={t('chat.workspace.close')}
                >
                  <CloseIcon />
                </button>
              </div>

              <form
                className={styles.pathForm}
                onSubmit={(event) => {
                  event.preventDefault();
                  const path = pathDraft.trim();
                  if (path) void load(path);
                }}
              >
                <label htmlFor="workspace-browser-path">
                  {t('chat.workspace.path')}
                </label>
                <input
                  ref={pathInputRef}
                  id="workspace-browser-path"
                  value={pathDraft}
                  onChange={(event) => setPathDraft(event.target.value)}
                  disabled={loading}
                  placeholder={t('chat.workspace.path')}
                  spellCheck={false}
                />
                <button type="submit" disabled={loading || !pathDraft.trim()}>
                  {t('chat.workspace.go')}
                </button>
              </form>

              <div className={styles.directoryList} aria-busy={loading}>
                {loading ? (
                  <div className={styles.state}>
                    {t('chat.workspace.loading')}
                  </div>
                ) : error ? (
                  <div className={styles.error} role="alert">
                    {error}
                  </div>
                ) : (
                  <>
                    {listing?.parent && (
                      <button
                        type="button"
                        className={styles.directory}
                        onClick={() => void load(listing.parent ?? undefined)}
                      >
                        <span className={styles.directoryIcon}>
                          <ArrowUpIcon />
                        </span>
                        <span className={styles.directoryText}>
                          <strong>{t('chat.workspace.parent')}</strong>
                          <small>{listing.parent}</small>
                        </span>
                      </button>
                    )}
                    {listing?.directories.map((directory) => (
                      <button
                        type="button"
                        className={styles.directory}
                        key={directory.path}
                        onClick={() => void load(directory.path)}
                      >
                        <span className={styles.directoryIcon}>
                          <FolderIcon />
                        </span>
                        <span className={styles.directoryText}>
                          <strong>{directory.name}</strong>
                          <small>{directory.path}</small>
                        </span>
                        <ChevronIcon className={styles.directoryChevron} />
                      </button>
                    ))}
                    {listing !== null && listing.directories.length === 0 && (
                      <div className={styles.state}>
                        {t('chat.workspace.empty')}
                      </div>
                    )}
                  </>
                )}
              </div>

              {listing?.truncated && !loading && !error && (
                <div className={styles.truncated} role="status">
                  {t('chat.workspace.truncated')}
                </div>
              )}

              <div className={styles.footer}>
                <div className={styles.currentWorkspace} title={listing?.path}>
                  <span>{t('chat.workspace.current')}</span>
                  <strong>{listing?.path ?? workspace}</strong>
                </div>
                <div className={styles.footerActions}>
                  <button
                    type="button"
                    className={styles.cancelButton}
                    onClick={closeBrowser}
                  >
                    {t('chat.workspace.cancel')}
                  </button>
                  <button
                    type="button"
                    className={styles.useButton}
                    disabled={loading || listing === null}
                    onClick={() => {
                      if (!listing) return;
                      closeBrowser();
                      selectWorkspace(listing.path);
                    }}
                  >
                    {t('chat.workspace.use')}
                  </button>
                </div>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}

function rememberWorkspace(
  workspaces: readonly string[],
  workspace: string | null,
): string[] {
  if (!workspace) return [...workspaces];
  return [workspace, ...workspaces.filter((item) => item !== workspace)].slice(
    0,
    MAX_RECENT_WORKSPACES,
  );
}

function readRecentWorkspaces(): string[] {
  try {
    const value = localStorage.getItem(RECENT_WORKSPACES_KEY);
    if (!value) return [];
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (item): item is string =>
          typeof item === 'string' && item.trim().length > 0,
      )
      .slice(0, MAX_RECENT_WORKSPACES);
  } catch {
    return [];
  }
}

function writeRecentWorkspaces(workspaces: readonly string[]): void {
  try {
    localStorage.setItem(
      RECENT_WORKSPACES_KEY,
      JSON.stringify(workspaces.slice(0, MAX_RECENT_WORKSPACES)),
    );
  } catch {
    // Workspace selection remains functional when storage is unavailable.
  }
}
