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

/** Mirrors `ServiceSource` in `api/src/services/railway.rs`. */
export type NewServiceSource = { docker_image: string } | { github_repo: string };

/**
 * A session whose Railway token the API could not use.
 *
 * Distinct from every other failure because only a new login fixes it: the
 * cookie is still good, the credential behind it is not.
 */
export class SessionExpired extends Error {}

/**
 * A request the API understood and declined, carrying the envelope's message —
 * which is worth showing, unlike a transport failure's.
 */
export class RequestRejected extends Error {}

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

/**
 * Creates a service in a project, from a Docker image or a GitHub repo — the
 * only two sources the API accepts. Railway picks the name and everything
 * else; the created service comes back as the API recorded it.
 */
export async function createService(
  projectId: string,
  source: NewServiceSource,
): Promise<Service> {
  const response = await fetch(
    `${API_URL}/api/v1/projects/${encodeURIComponent(projectId)}/services`,
    {
      method: 'POST',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ source }),
    },
  );

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  if (!response.ok) {
    throw new RequestRejected(await rejectionMessage(response));
  }

  return (await response.json()) as Service;
}

/** The envelope's message when there is one, a generic line when there is not. */
async function rejectionMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: { message?: string } };
    if (body.error?.message) return body.error.message;
  } catch {
    // A non-JSON body falls through to the generic message.
  }

  return `The service could not be created (${response.status}).`;
}
