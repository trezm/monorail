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

/** Mirrors `Environment` in `api/src/services/railway.rs`. */
export interface Environment {
  id: string;
  name: string;
  created_at: string | null;
}

/** Mirrors `Deployment` in `api/src/services/railway.rs`. */
export interface Deployment {
  id: string;
  status: string;
  created_at: string | null;
}

/** Mirrors `ServiceInstance` in `api/src/services/railway.rs`. */
export interface ServiceInstance {
  id: string;
  start_command: string | null;
  build_command: string | null;
  root_directory: string | null;
  healthcheck_path: string | null;
  region: string | null;
  num_replicas: number | null;
  restart_policy_type: string | null;
  restart_policy_max_retries: number | null;
  latest_deployment: Deployment | null;
}

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
 * One authenticated GET.
 *
 * `credentials: 'include'` is what attaches the session cookie across origins;
 * the API allows it for these.
 */
async function request(path: string): Promise<Response> {
  const response = await fetch(`${API_URL}${path}`, {
    credentials: 'include',
  });

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  return response;
}

/** Every project on the account, each with its services nested in it. */
export async function projects(): Promise<Project[]> {
  const response = await request('/api/v1/projects');

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

/** One project's durable environments; the per-pull-request ones are excluded. */
export async function environments(projectId: string): Promise<Environment[]> {
  const response = await request(
    `/api/v1/projects/${encodeURIComponent(projectId)}/environments`,
  );

  if (!response.ok) {
    throw new Error(`GET /api/v1/projects/{id}/environments answered ${response.status}`);
  }

  const body = (await response.json()) as { environments: Environment[] };

  return body.environments;
}

/**
 * How a service is configured in one environment, or `null` when it has no
 * instance there — a service does not have to be deployed everywhere its
 * project has environments.
 */
export async function serviceInstance(
  serviceId: string,
  environmentId: string,
): Promise<ServiceInstance | null> {
  const response = await request(
    `/api/v1/services/${encodeURIComponent(serviceId)}/instance?environment=${encodeURIComponent(environmentId)}`,
  );

  if (response.status === 404) return null;

  if (!response.ok) {
    throw new Error(`GET /api/v1/services/{id}/instance answered ${response.status}`);
  }

  return (await response.json()) as ServiceInstance;
}
