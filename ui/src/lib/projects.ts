/** The signed-in user's Railway projects. */

import { API_URL } from './session';

/** Mirrors `Service` in `api/src/services/railway.rs`. */
export interface Service {
  id: string;
  name: string;
  created_at: string | null;
}

/** Mirrors `Project` in `api/src/services/railway.rs`. */
export interface Project {
  id: string;
  name: string;
  description: string | null;
  created_at: string | null;
  services: Service[];
}

/**
 * A session whose Railway token the API could not use.
 *
 * Distinct from every other failure because only a new login fixes it: the
 * cookie is still good, the credential behind it is not.
 */
export class SessionExpired extends Error {}

/**
 * Every project on the account, each with its services nested in it.
 *
 * `credentials: 'include'` is what attaches the session cookie across origins;
 * the API allows it for this one.
 */
export async function projects(): Promise<Project[]> {
  const response = await fetch(`${API_URL}/api/v1/projects`, {
    credentials: 'include',
  });

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  if (!response.ok) {
    throw new Error(`GET /api/v1/projects answered ${response.status}`);
  }

  const body = (await response.json()) as { projects: Project[] };

  return body.projects;
}
