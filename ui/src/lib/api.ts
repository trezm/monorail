/**
 * The API client. One module so every call agrees on the base URL, on sending
 * the session cookie, and on how a failure is shaped.
 */

// Only reached by a plain `pnpm dev` with no `.env`. Every Bazel build is
// given the value, and one given an empty value fails in astro.config.mjs
// rather than shipping this.
export const API_URL = import.meta.env.PUBLIC_API_URL ?? 'http://localhost:8080';

export const LOGIN_URL = `${API_URL}/auth/railway`;

export interface Profile {
  id: string;
  email: string | null;
  name: string | null;
  avatar_url: string | null;
}

export interface Service {
  id: string;
  name: string;
  created_at: string | null;
}

export interface Project {
  id: string;
  name: string;
  description: string | null;
  created_at: string | null;
  services: Service[];
}

/**
 * A failed call, carrying the API's own error code.
 *
 * Branch on `code`, never on `message` — that is the contract the API states.
 * `unreachable` is the one code invented here, for a request that never got an
 * answer at all.
 */
export class ApiError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }

  get isUnauthorized() {
    return this.code === 'unauthorized';
  }
}

/**
 * `credentials: 'include'` is what sends the session cookie, which the API sets
 * on its own origin rather than the UI's. It only works against an API whose
 * `API_CORS_ALLOWED_ORIGINS` names this one: a wildcard cannot allow
 * credentials.
 */
async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  let response: Response;

  try {
    response = await fetch(`${API_URL}${path}`, {
      ...init,
      credentials: 'include',
      headers: { Accept: 'application/json', ...init.headers },
    });
  } catch {
    throw new ApiError('unreachable', `${API_URL} could not be reached`);
  }

  if (!response.ok) {
    const body = await response.json().catch(() => null);

    throw new ApiError(
      body?.error?.code ?? 'unknown_error',
      body?.error?.message ?? `the API answered ${response.status}`,
    );
  }

  return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
}

/** Who is logged in, or an `unauthorized` [`ApiError`] if nobody is. */
export function getProfile() {
  return request<Profile>('/api/v1/users/me');
}

/** Every Railway project on the account, each with its services nested in it. */
export async function getProjects() {
  const { projects } = await request<{ projects: Project[] }>('/api/v1/projects');

  return projects;
}

export function logOut() {
  return request<void>('/auth/session', { method: 'DELETE' });
}
