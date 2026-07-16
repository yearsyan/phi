/**
 * Client-side configuration backed by `localStorage`.
 *
 * The daemon key is the long-lived bearer used for HTTP; WebSocket connections
 * exchange it for a single-use token via {@link fetchWsToken}. The selected
 * profile id chooses which Provider profile backs new sessions.
 */

const KEY_DAEMON_AUTH = 'phi.daemon.authKey';
const KEY_PROFILE_ID = 'phi.daemon.profileId';

export interface DaemonConfig {
  authKey: string;
  profileId: string;
}

export function readDaemonConfig(): DaemonConfig {
  const authKey =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem(KEY_DAEMON_AUTH) ?? '')
      : '';
  const profileId =
    typeof localStorage !== 'undefined'
      ? (localStorage.getItem(KEY_PROFILE_ID) ?? 'default')
      : 'default';
  return { authKey, profileId };
}

export function writeAuthKey(value: string): void {
  if (value) {
    localStorage.setItem(KEY_DAEMON_AUTH, value);
  } else {
    localStorage.removeItem(KEY_DAEMON_AUTH);
  }
}

export function writeProfileId(value: string): void {
  const trimmed = value.trim() || 'default';
  localStorage.setItem(KEY_PROFILE_ID, trimmed);
}

export function isConfigured(config: DaemonConfig): boolean {
  return config.authKey.trim().length > 0;
}
