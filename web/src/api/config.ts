/**
 * Client-side configuration.
 *
 * The daemon key is the long-lived bearer used for HTTP; WebSocket connections
 * exchange it for a single-use token via {@link fetchWsToken}. The selected
 * profile ids and capability mode choose the defaults for new sessions.
 *
 * Storage split:
 * - The auth key lives in `localStorage`, so it persists across tabs and
 *   browser restarts for the same origin. Browser architecture means any
 *   in-page JS can read it, so it must still be treated as a high-privilege
 *   credential rather than a sandboxed secret.
 * - Non-sensitive UI prefs (profile ids, capability mode) also stay in
 *   `localStorage`.
 */

import type { CapabilityMode } from '../types/wire.ts';

const KEY_DAEMON_AUTH = 'phi.daemon.authKey';
const KEY_PROFILE_ID = 'phi.daemon.profileId';
const KEY_AGENT_PROFILE_ID = 'phi.daemon.agentProfileId';
const KEY_CAPABILITY_MODE = 'phi.daemon.capabilityMode';

export interface DaemonConfig {
  authKey: string;
  profileId: string;
  agentProfileId: string;
  capabilityMode: CapabilityMode | null;
}

/**
 * One-time migration from the previous `sessionStorage` auth-key slot to
 * `localStorage`. An existing local key wins over a stale tab-scoped copy, and
 * the session copy is always removed after migration.
 */
function migrateAuthKeyToLocalStorage(): void {
  if (
    typeof localStorage === 'undefined' ||
    typeof sessionStorage === 'undefined'
  ) {
    return;
  }
  const sessionKey = sessionStorage.getItem(KEY_DAEMON_AUTH);
  if (sessionKey && !localStorage.getItem(KEY_DAEMON_AUTH)) {
    localStorage.setItem(KEY_DAEMON_AUTH, sessionKey);
  }
  sessionStorage.removeItem(KEY_DAEMON_AUTH);
}

export function readDaemonConfig(): DaemonConfig {
  migrateAuthKeyToLocalStorage();
  const authKey =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem(KEY_DAEMON_AUTH) ?? '')
      : '';
  const profileId =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem(KEY_PROFILE_ID) ?? 'default')
      : 'default';
  const agentProfileId =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem(KEY_AGENT_PROFILE_ID) ?? '')
      : '';
  const storedCapabilityMode =
    typeof localStorage !== 'undefined'
      ? localStorage.getItem(KEY_CAPABILITY_MODE)
      : null;
  const capabilityMode = isCapabilityMode(storedCapabilityMode)
    ? storedCapabilityMode
    : null;
  return { authKey, profileId, agentProfileId, capabilityMode };
}

export function writeAuthKey(value: string): void {
  if (typeof localStorage === 'undefined') return;
  if (value) {
    localStorage.setItem(KEY_DAEMON_AUTH, value);
  } else {
    localStorage.removeItem(KEY_DAEMON_AUTH);
  }
  if (typeof sessionStorage !== 'undefined') {
    sessionStorage.removeItem(KEY_DAEMON_AUTH);
  }
}

export function writeProfileId(value: string): void {
  const trimmed = value.trim() || 'default';
  localStorage.setItem(KEY_PROFILE_ID, trimmed);
}

export function writeAgentProfileId(value: string): void {
  const trimmed = value.trim();
  if (trimmed) {
    localStorage.setItem(KEY_AGENT_PROFILE_ID, trimmed);
  } else {
    localStorage.removeItem(KEY_AGENT_PROFILE_ID);
  }
}

export function writeCapabilityMode(value: CapabilityMode | null): void {
  if (value === null) {
    localStorage.removeItem(KEY_CAPABILITY_MODE);
  } else {
    localStorage.setItem(KEY_CAPABILITY_MODE, value);
  }
}

export function isConfigured(config: DaemonConfig): boolean {
  return config.authKey.trim().length > 0;
}

function isCapabilityMode(value: string | null): value is CapabilityMode {
  return (
    value === 'read_only' ||
    value === 'workspace_edit' ||
    value === 'full_access'
  );
}
