import { useCallback, useEffect, useRef, useState } from 'react';
import { listSessions } from '../api/http.ts';
import type { SessionSummary } from '../types/wire.ts';

export interface SessionListState {
  sessions: SessionSummary[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

/**
 * Loads `GET /v1/sessions` once when enabled. Later refreshes are explicit so
 * background polling cannot make the sidebar flicker or issue idle requests.
 */
export function useSessionList(
  authKey: string,
  enabled: boolean,
): SessionListState {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const authKeyRef = useRef(authKey);
  const requestRevisionRef = useRef(0);
  authKeyRef.current = authKey;

  const refresh = useCallback(async () => {
    const revision = ++requestRevisionRef.current;
    const key = authKeyRef.current;
    if (!key) {
      setSessions([]);
      setError(null);
      return;
    }
    setLoading(true);
    try {
      const response = await listSessions(key);
      if (revision !== requestRevisionRef.current) return;
      setSessions(response.sessions);
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
      setSessions([]);
      setError(null);
      setLoading(false);
      return;
    }
    void refresh();
    return () => {
      requestRevisionRef.current += 1;
    };
  }, [enabled, refresh]);

  return { sessions, loading, error, refresh };
}
