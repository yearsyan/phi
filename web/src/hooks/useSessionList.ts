import { useCallback, useEffect, useRef, useState } from 'react';
import { listSessions } from '../api/http.ts';
import type { SessionSummary } from '../types/wire.ts';

const POLL_INTERVAL_MS = 2500;

export interface SessionListState {
  sessions: SessionSummary[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

/**
 * Polls `GET /v1/sessions` on an interval while `enabled`. Errors are surfaced
 * but do not stop polling, so the list recovers once the daemon/auth is fixed.
 */
export function useSessionList(
  authKey: string,
  enabled: boolean,
): SessionListState {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const authKeyRef = useRef(authKey);
  authKeyRef.current = authKey;

  const refresh = useCallback(async () => {
    const key = authKeyRef.current;
    if (!key) {
      setSessions([]);
      setError(null);
      return;
    }
    setLoading(true);
    try {
      const response = await listSessions(key);
      setSessions(response.sessions);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const handle = window.setInterval(() => {
      void refresh();
    }, POLL_INTERVAL_MS);
    return () => window.clearInterval(handle);
  }, [enabled, refresh]);

  return { sessions, loading, error, refresh };
}
