export type AppRoute =
  | { kind: 'new_session' }
  | { kind: 'scheduled_tasks' }
  | { kind: 'session'; sessionId: string };

export const NEW_SESSION_PATH = '/sessions/new';
export const SCHEDULED_TASKS_PATH = '/scheduled-tasks';

/** Parse a browser pathname into one of the public Web client routes. */
export function parseAppRoute(pathname: string): AppRoute | null {
  const normalized = normalizePathname(pathname);
  if (normalized === '/' || normalized === NEW_SESSION_PATH) {
    return { kind: 'new_session' };
  }
  if (normalized === SCHEDULED_TASKS_PATH) {
    return { kind: 'scheduled_tasks' };
  }

  const sessionPrefix = '/sessions/';
  if (!normalized.startsWith(sessionPrefix)) return null;
  const encodedSessionId = normalized.slice(sessionPrefix.length);
  if (!encodedSessionId || encodedSessionId.includes('/')) return null;

  try {
    const sessionId = decodeURIComponent(encodedSessionId);
    if (!sessionId || sessionId === 'new') return null;
    return { kind: 'session', sessionId };
  } catch {
    return null;
  }
}

/** Return the canonical pathname for a Web client route. */
export function appRoutePath(route: AppRoute): string {
  switch (route.kind) {
    case 'new_session':
      return NEW_SESSION_PATH;
    case 'scheduled_tasks':
      return SCHEDULED_TASKS_PATH;
    case 'session':
      return `/sessions/${encodeURIComponent(route.sessionId)}`;
  }
}

/**
 * Update the address bar without reloading the SPA.
 *
 * Replacing is used when a prepared session receives its durable session id;
 * explicit user navigation pushes a new history entry.
 */
export function writeAppRoute(
  route: AppRoute,
  options: { replace?: boolean } = {},
): void {
  if (typeof window === 'undefined') return;
  const pathname = appRoutePath(route);
  if (window.location.pathname === pathname) return;
  const method = options.replace ? 'replaceState' : 'pushState';
  window.history[method](null, '', pathname);
}

function normalizePathname(pathname: string): string {
  if (!pathname) return '/';
  if (pathname.length > 1 && pathname.endsWith('/')) {
    return pathname.slice(0, -1);
  }
  return pathname;
}
