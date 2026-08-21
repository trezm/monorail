/** A service's horizontal autoscaling rules. */

import { API_URL } from './session';
import { RequestRejected, SessionExpired } from './projects';

/** Mirrors `Metric` in `api/src/services/autoscaling.rs`. */
export type Metric = 'CPU' | 'MEMORY' | 'NETWORK_RX' | 'NETWORK_TX';

/**
 * Mirrors `Rule` in `api/src/services/autoscaling.rs`. A service takes one
 * rule, so its identity is the service alone. The thresholds bound the
 * metric; min_count/max_count bound the replica count the loop may steer to.
 */
export interface Rule {
  service_id: string;
  metric: Metric;
  environment_id: string;
  min_threshold: number;
  max_threshold: number;
  min_count: number;
  max_count: number;
  poll_frequency_secs: number;
  last_checked: string | null;
  created_at: string;
  updated_at: string;
}

/** Mirrors `NewRule` in `api/src/services/autoscaling.rs`. */
export interface NewRule {
  environment_id: string;
  metric: Metric;
  min_threshold: number;
  max_threshold: number;
  min_count: number;
  max_count: number;
  poll_frequency_secs: number;
}

/** The service's rule, as a list of at most one. */
export async function rules(serviceId: string): Promise<Rule[]> {
  const response = await fetch(
    `${API_URL}/api/v1/services/${encodeURIComponent(serviceId)}/autoscaling`,
    { credentials: 'include' },
  );

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  if (!response.ok) {
    throw new Error(`GET /api/v1/services/{id}/autoscaling answered ${response.status}`);
  }

  const body = (await response.json()) as { rules: Rule[] };

  return body.rules;
}

/**
 * Adds a rule to a service. A service takes one rule; a second is declined
 * with the envelope's message, which is worth showing.
 */
export async function createRule(serviceId: string, rule: NewRule): Promise<Rule> {
  const response = await fetch(
    `${API_URL}/api/v1/services/${encodeURIComponent(serviceId)}/autoscaling`,
    {
      method: 'POST',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(rule),
    },
  );

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  if (!response.ok) {
    throw new RequestRejected(await rejectionMessage(response, 'created'));
  }

  return (await response.json()) as Rule;
}

/** Removes the service's rule. Removing one that is already gone is a rejection, not a success. */
export async function removeRule(serviceId: string): Promise<void> {
  const response = await fetch(
    `${API_URL}/api/v1/services/${encodeURIComponent(serviceId)}/autoscaling`,
    { method: 'DELETE', credentials: 'include' },
  );

  if (response.status === 401) {
    throw new SessionExpired('the Railway login behind this session has expired');
  }

  if (!response.ok) {
    throw new RequestRejected(await rejectionMessage(response, 'removed'));
  }
}

/** The envelope's message when there is one, a generic line when there is not. */
async function rejectionMessage(response: Response, action: string): Promise<string> {
  try {
    const body = (await response.json()) as { error?: { message?: string } };
    if (body.error?.message) return body.error.message;
  } catch {
    // A non-JSON body falls through to the generic message.
  }

  return `The rule could not be ${action} (${response.status}).`;
}
