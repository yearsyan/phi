import type { AuthTokenResponse } from '../types/wire.ts';

export class AuthError extends Error {
  readonly code: string;
  constructor(message: string, code = 'unauthorized') {
    super(message);
    this.name = 'AuthError';
    this.code = code;
  }
}

/**
 * Exchange the long-lived daemon key for a single-use WebSocket token.
 *
 * POST /v1/auth/token with `Authorization: Bearer <daemon-key>`. The returned
 * token is only valid for one WebSocket upgrade attempt and expires quickly.
 */
export async function fetchWsToken(
  authKey: string,
  signal?: AbortSignal,
): Promise<AuthTokenResponse> {
  const response = await fetch('/v1/auth/token', {
    method: 'POST',
    signal,
    headers: {
      Authorization: `Bearer ${authKey}`,
    },
  });
  if (!response.ok) {
    throw new AuthError(
      `Failed to obtain WebSocket token (${response.status})`,
      response.status === 401 ? 'unauthorized' : 'token_failed',
    );
  }
  return (await response.json()) as AuthTokenResponse;
}
