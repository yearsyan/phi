import type {
  ProviderResponse,
  ProvidersResponse,
  PutProviderRequest,
  SessionSummary,
  SessionsResponse,
} from '../types/wire.ts';
import { AuthError } from './token.ts';

async function http<T>(
  path: string,
  authKey: string,
  init: RequestInit = {},
): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      Authorization: `Bearer ${authKey}`,
      ...(init.body ? { 'content-type': 'application/json' } : {}),
      ...(init.headers ?? {}),
    },
  });
  if (response.status === 401) {
    throw new AuthError('Daemon rejected the auth key', 'unauthorized');
  }
  if (!response.ok) {
    let detail = '';
    try {
      const body = await response.json();
      detail = body?.message ? `: ${body.message}` : '';
    } catch {
      /* ignore non-JSON error bodies */
    }
    throw new Error(`HTTP ${response.status}${detail}`);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export function listSessions(authKey: string): Promise<SessionsResponse> {
  return http<SessionsResponse>('/v1/sessions', authKey, { method: 'GET' });
}

export function getSession(
  authKey: string,
  sessionId: string,
): Promise<SessionSummary> {
  return http<SessionSummary>(
    `/v1/sessions/${encodeURIComponent(sessionId)}`,
    authKey,
    { method: 'GET' },
  );
}

export function listProviders(authKey: string): Promise<ProvidersResponse> {
  return http<ProvidersResponse>('/v1/providers', authKey, { method: 'GET' });
}

export function getProvider(
  authKey: string,
  profileId: string,
): Promise<ProviderResponse> {
  return http<ProviderResponse>(
    `/v1/providers/${encodeURIComponent(profileId)}`,
    authKey,
    { method: 'GET' },
  );
}

export function putProvider(
  authKey: string,
  profileId: string,
  body: PutProviderRequest,
): Promise<ProviderResponse> {
  return http<ProviderResponse>(
    `/v1/providers/${encodeURIComponent(profileId)}`,
    authKey,
    { method: 'PUT', body: JSON.stringify(body) },
  );
}
