import type {
  SessionSocketHandlers,
  SessionSocketOpenOptions,
} from './connection.ts';
import { SessionSocket } from './connection.ts';

/**
 * Open a brand-new prepared session. `profile_id` defaults to "default".
 * The connection goes through `building` then `ready`; no session id exists
 * until the first `prompt` produces `session_created`.
 */
export function openNewSession(
  authKey: string,
  profileId: string,
  handlers: SessionSocketHandlers,
  options?: SessionSocketOpenOptions,
): Promise<SessionSocket> {
  const path = `/v1/ws/new?profile_id=${encodeURIComponent(profileId)}`;
  return SessionSocket.open(path, authKey, handlers, options);
}

/**
 * Attach to an existing session (online or offline) by id. The first server
 * frame is a full `snapshot` the client uses to seed its projection.
 */
export function attachSession(
  authKey: string,
  sessionId: string,
  handlers: SessionSocketHandlers,
  options?: SessionSocketOpenOptions,
): Promise<SessionSocket> {
  const path = `/v1/ws/attach/${encodeURIComponent(sessionId)}`;
  return SessionSocket.open(path, authKey, handlers, options);
}
