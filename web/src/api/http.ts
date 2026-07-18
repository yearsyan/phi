import type {
  AgentProfileResponse,
  AgentProfilesResponse,
  CreateScheduledTaskRequest,
  ForkPosition,
  ProviderResponse,
  ProvidersResponse,
  PutAgentProfileRequest,
  PutProviderRequest,
  ScheduledTask,
  ScheduledTasksResponse,
  SessionSummary,
  SessionsResponse,
  WorkspaceBrowseResponse,
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
  if (response.status === 202 || response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export function listSessions(authKey: string): Promise<SessionsResponse> {
  return http<SessionsResponse>('/v1/sessions', authKey, { method: 'GET' });
}

export function browseWorkspace(
  authKey: string,
  path?: string,
): Promise<WorkspaceBrowseResponse> {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  const query = params.size > 0 ? `?${params.toString()}` : '';
  return http<WorkspaceBrowseResponse>(
    `/v1/workspaces/browse${query}`,
    authKey,
    { method: 'GET' },
  );
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

export function setSessionPinned(
  authKey: string,
  sessionId: string,
  pinned: boolean,
): Promise<SessionSummary> {
  return http<SessionSummary>(
    `/v1/sessions/${encodeURIComponent(sessionId)}`,
    authKey,
    { method: 'PATCH', body: JSON.stringify({ pinned }) },
  );
}

export function deleteSession(
  authKey: string,
  sessionId: string,
): Promise<void> {
  return http<void>(`/v1/sessions/${encodeURIComponent(sessionId)}`, authKey, {
    method: 'DELETE',
  });
}

export function listScheduledTasks(
  authKey: string,
): Promise<ScheduledTasksResponse> {
  return http<ScheduledTasksResponse>('/v1/scheduled-tasks', authKey, {
    method: 'GET',
  });
}

export function createScheduledTask(
  authKey: string,
  body: CreateScheduledTaskRequest,
): Promise<ScheduledTask> {
  return http<ScheduledTask>('/v1/scheduled-tasks', authKey, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function updateScheduledTask(
  authKey: string,
  taskId: string,
  enabled: boolean,
  expectedRevision: number,
): Promise<ScheduledTask> {
  return http<ScheduledTask>(
    `/v1/scheduled-tasks/${encodeURIComponent(taskId)}`,
    authKey,
    {
      method: 'PATCH',
      body: JSON.stringify({
        enabled,
        expected_revision: expectedRevision,
      }),
    },
  );
}

export function runScheduledTask(
  authKey: string,
  taskId: string,
): Promise<void> {
  return http<void>(
    `/v1/scheduled-tasks/${encodeURIComponent(taskId)}/run`,
    authKey,
    { method: 'POST' },
  );
}

export function deleteScheduledTask(
  authKey: string,
  taskId: string,
): Promise<void> {
  return http<void>(
    `/v1/scheduled-tasks/${encodeURIComponent(taskId)}`,
    authKey,
    { method: 'DELETE' },
  );
}

export function forkSession(
  authKey: string,
  sessionId: string,
  messageIndex: number,
  position: ForkPosition = 'after',
): Promise<SessionSummary> {
  const body =
    position === 'after'
      ? { message_index: messageIndex }
      : { message_index: messageIndex, position };
  return http<SessionSummary>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/fork`,
    authKey,
    { method: 'POST', body: JSON.stringify(body) },
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

export function listAgentProfiles(
  authKey: string,
): Promise<AgentProfilesResponse> {
  return http<AgentProfilesResponse>('/v1/agent-profiles', authKey, {
    method: 'GET',
  });
}

export function getAgentProfile(
  authKey: string,
  agentProfileId: string,
): Promise<AgentProfileResponse> {
  return http<AgentProfileResponse>(
    `/v1/agent-profiles/${encodeURIComponent(agentProfileId)}`,
    authKey,
    { method: 'GET' },
  );
}

export function putAgentProfile(
  authKey: string,
  agentProfileId: string,
  body: PutAgentProfileRequest,
): Promise<AgentProfileResponse> {
  return http<AgentProfileResponse>(
    `/v1/agent-profiles/${encodeURIComponent(agentProfileId)}`,
    authKey,
    { method: 'PUT', body: JSON.stringify(body) },
  );
}
