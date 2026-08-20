/** A service's horizontal autoscaling rules. */

import { API_URL } from './session';
import { RequestRejected, SessionExpired } from './projects';

/** Mirrors `Metric` in `api/src/services/autoscaling.rs`. */
export type Metric = 'CPU' | 'MEMORY' | 'NETWORK_RX' | 'NETWORK_TX';

/** Mirrors `Rule` in `api/src/services/autoscaling.rs`. */
export interface Rule {
  id: string;
  service_id: string;
  environment_id: string;
  metric: Metric;
  min_threshold: number;
  max_threshold: number;
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
  poll_frequency_secs: number;
}

/** The rules watching one service, oldest first. */
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
 * Adds a rule to a service. A service takes one rule per metric; a second is
 * declined with the envelope's message, which is worth showing.
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

/** Removes a rule. Removing one that is already gone is a rejection, not a success. */
export async function removeRule(serviceId: string, ruleId: string): Promise<void> {
  const response = await fetch(
    `${API_URL}/api/v1/services/${encodeURIComponent(serviceId)}/autoscaling/${encodeURIComponent(ruleId)}`,
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
