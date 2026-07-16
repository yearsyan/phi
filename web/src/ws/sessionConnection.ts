import type { CapabilityMode } from '../types/wire.ts';
import type {
  SessionSocketHandlers,
  SessionSocketOpenOptions,
} from './connection.ts';
import { SessionSocket } from './connection.ts';

export interface NewSessionSocketOpenOptions extends SessionSocketOpenOptions {
  agentProfileId?: string;
  capabilityMode?: CapabilityMode;
}

/**
 * Open a brand-new prepared session. `profile_id` defaults to "default".
 * The connection goes through `building` then `ready`; no session id exists
 * until the first `prompt` produces `session_created`.
 */
export function openNewSession(
  authKey: string,
  profileId: string,
  handlers: SessionSocketHandlers,
  options?: NewSessionSocketOpenOptions,
): Promise<SessionSocket> {
  const params = new URLSearchParams({ profile_id: profileId });
  const agentProfileId = options?.agentProfileId?.trim();
  if (agentProfileId) params.set('agent_profile_id', agentProfileId);
  if (options?.capabilityMode) {
    params.set('capability_mode', options.capabilityMode);
  }
  const path = `/v1/ws/new?${params.toString()}`;
  const socketOptions =
    options?.signal === undefined ? undefined : { signal: options.signal };
  return SessionSocket.open(path, authKey, handlers, socketOptions);
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
