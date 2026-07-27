/**
 * Client-side configuration.
 *
 * The daemon key is the long-lived bearer used for HTTP; WebSocket connections
 * exchange it for a single-use token via {@link fetchWsToken}. The selected
 * profile ids and capability mode choose the defaults for new sessions.
 *
 * Storage split:
 * - The auth key lives in `sessionStorage`. It is scoped to this tab and is
 *   dropped when the tab closes, so the long-lived credential is never
 *   persisted to disk beyond the browsing session. (Browser architecture
 *   means any in-page JS can still read it during the session — this is a
 *   reduction of exposure window, not a sandbox.)
 * - Non-sensitive UI prefs (profile ids, capability mode) stay in
 *   `localStorage` so they persist across sessions.
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
 * One-time migration from the legacy `localStorage` auth-key slot to
 * `sessionStorage`. If a key is present in `localStorage` and absent from
 * `sessionStorage`, it is moved across and removed from `localStorage`. Safe
 * to call on every load; a no-op once migration has run.
 */
function migrateAuthKeyToSessionStorage(): void {
  if (
    typeof localStorage === 'undefined' ||
    typeof sessionStorage === 'undefined'
  ) {
    return;
  }
  const legacy = localStorage.getItem(KEY_DAEMON_AUTH);
  if (legacy && !sessionStorage.getItem(KEY_DAEMON_AUTH)) {
    sessionStorage.setItem(KEY_DAEMON_AUTH, legacy);
    localStorage.removeItem(KEY_DAEMON_AUTH);
  } else if (legacy) {
    // Already migrated (sessionStorage has a value); drop the stale legacy
    // copy so it can no longer be read from `localStorage`.
    localStorage.removeItem(KEY_DAEMON_AUTH);
  }
}

export function readDaemonConfig(): DaemonConfig {
  migrateAuthKeyToSessionStorage();
  const authKey =
    typeof sessionStorage !== 'undefined'
      ? (sessionStorage.getItem(KEY_DAEMON_AUTH) ?? '')
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
  if (typeof sessionStorage === 'undefined') return;
  if (value) {
    sessionStorage.setItem(KEY_DAEMON_AUTH, value);
  } else {
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
