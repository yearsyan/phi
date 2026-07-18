import { useCallback, useEffect, useRef, useState } from 'react';
import {
  deleteSession as deleteSessionRequest,
  listSessions,
  setSessionPinned as setSessionPinnedRequest,
} from '../api/http.ts';
import type { SessionsResponse, WorkspaceSessionGroup } from '../types/wire.ts';

export interface SessionListState {
  workspaces: WorkspaceSessionGroup[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  setPinned: (sessionId: string, pinned: boolean) => Promise<void>;
  deleteSession: (sessionId: string) => Promise<void>;
}

/**
 * Loads `GET /v1/sessions` once when enabled. Later refreshes are explicit so
 * background polling cannot make the sidebar flicker or issue idle requests.
 */
export function useSessionList(
  authKey: string,
  enabled: boolean,
): SessionListState {
  const [workspaces, setWorkspaces] = useState<WorkspaceSessionGroup[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const authKeyRef = useRef(authKey);
  const requestRevisionRef = useRef(0);
  authKeyRef.current = authKey;

  const refresh = useCallback(async () => {
    const revision = ++requestRevisionRef.current;
    const key = authKeyRef.current;
    if (!key) {
      setWorkspaces([]);
      setError(null);
      return;
    }
    setLoading(true);
    try {
      const response = await listSessions(key);
      if (revision !== requestRevisionRef.current) return;
      setWorkspaces(workspaceTree(response));
      setError(null);
    } catch (err) {
      if (revision !== requestRevisionRef.current) return;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (revision === requestRevisionRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) {
      requestRevisionRef.current += 1;
      setWorkspaces([]);
      setError(null);
      setLoading(false);
      return;
    }
    void refresh();
    return () => {
      requestRevisionRef.current += 1;
    };
  }, [enabled, refresh]);

  const setPinned = useCallback(async (sessionId: string, pinned: boolean) => {
    const revision = ++requestRevisionRef.current;
    const key = authKeyRef.current;
    setLoading(true);
    try {
      await setSessionPinnedRequest(key, sessionId, pinned);
      if (revision !== requestRevisionRef.current) return;
      const response = await listSessions(key);
      if (revision !== requestRevisionRef.current) return;
      setWorkspaces(workspaceTree(response));
      setError(null);
    } catch (err) {
      if (revision === requestRevisionRef.current) {
        setError(err instanceof Error ? err.message : String(err));
      }
      throw err;
    } finally {
      if (revision === requestRevisionRef.current) setLoading(false);
    }
  }, []);

  const deleteSession = useCallback(async (sessionId: string) => {
    const revision = ++requestRevisionRef.current;
    const key = authKeyRef.current;
    setLoading(true);
    try {
      await deleteSessionRequest(key, sessionId);
      if (revision !== requestRevisionRef.current) return;
      const response = await listSessions(key);
      if (revision !== requestRevisionRef.current) return;
      setWorkspaces(workspaceTree(response));
      setError(null);
    } catch (err) {
      if (revision === requestRevisionRef.current) {
        setError(err instanceof Error ? err.message : String(err));
      }
      throw err;
    } finally {
      if (revision === requestRevisionRef.current) setLoading(false);
    }
  }, []);

  return { workspaces, loading, error, refresh, setPinned, deleteSession };
}

function workspaceTree(response: SessionsResponse): WorkspaceSessionGroup[] {
  if (response.workspaces) return response.workspaces;
  // Compatibility with older daemons: retain their flat order without doing
  // workspace clustering in the client.
  return response.sessions.length === 0
    ? []
    : [{ workspace: null, sessions: response.sessions }];
}
