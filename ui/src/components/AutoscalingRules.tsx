import { useEffect, useId, useState, type FormEvent } from 'react';

import {
  createRule,
  removeRule,
  rules as fetchRules,
  type Metric,
  type Rule,
} from '../lib/autoscaling';
import { RequestRejected, SessionExpired } from '../lib/projects';
import { useSession } from '../lib/session';

/** The unit each metric's thresholds are read in. */
const METRICS: Record<Metric, { label: string; unit: string }> = {
  CPU: { label: 'CPU', unit: 'vCPU' },
  MEMORY: { label: 'Memory', unit: 'GB' },
  NETWORK_RX: { label: 'Network in', unit: 'GB' },
  NETWORK_TX: { label: 'Network out', unit: 'GB' },
};

/**
 * One service's autoscaling rule: the one it has, or a form to add one — a
 * service takes a single rule, so the form and the rule never show together.
 * A new rule watches the environment currently picked in the dropdown above —
 * the rule remembers it, since scaling acts on a service *in* an environment.
 */
export default function AutoscalingRules({
  serviceId,
  environmentId,
}: {
  serviceId: string;
  environmentId: string;
}) {
  const session = useSession();
  const [loaded, setLoaded] = useState<Rule[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;

    fetchRules(serviceId)
      .then((found) => {
        if (live) setLoaded(found);
      })
      .catch((cause: unknown) => {
        if (!live) return;

        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => setFailed(true));
          return;
        }

        setFailed(true);
      });

    return () => {
      live = false;
    };
  }, [serviceId, session]);

  const remove = () => {
    setError(null);

    removeRule(serviceId)
      .then(() => {
        setLoaded([]);
      })
      .catch((cause: unknown) => {
        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => {});
          return;
        }

        setError(
          cause instanceof RequestRejected ? cause.message : 'The rule could not be removed.',
        );
      });
  };

  if (failed) {
    return (
      <p className="service__note service__note--error">
        The autoscaling rules could not be loaded.
      </p>
    );
  }
  if (loaded === null) return null;

  return (
    <div className="autoscaling">
      <h3 className="autoscaling__heading">Autoscaling</h3>

      {loaded.length > 0 ? (
        <ul className="autoscaling__rules">
          {loaded.map((rule) => (
            <li key={rule.service_id} className="autoscaling__rule">
              <span className="autoscaling__summary">
                {METRICS[rule.metric].label}: {rule.min_threshold}–{rule.max_threshold}{' '}
                {METRICS[rule.metric].unit} · {rule.min_count}–{rule.max_count} replicas · every{' '}
                {rule.poll_frequency_secs}s
              </span>
              <button
                type="button"
                className="autoscaling__remove"
                onClick={remove}
                aria-label={`Remove the ${METRICS[rule.metric].label} rule`}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <NewRuleForm
          serviceId={serviceId}
          environmentId={environmentId}
          onCreated={(rule) => setLoaded([rule])}
        />
      )}

      {error && (
        <p className="notice notice--error autoscaling__error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * Thresholds ride as strings until submit so a half-typed `0.` survives; the
 * API re-validates everything, so this only has to be honest, not exhaustive.
 */
function NewRuleForm({
  serviceId,
  environmentId,
  onCreated,
}: {
  serviceId: string;
  environmentId: string;
  onCreated: (rule: Rule) => void;
}) {
  const session = useSession();
  const id = useId();
  const [metric, setMetric] = useState<Metric>('CPU');
  const [min, setMin] = useState('');
  const [max, setMax] = useState('');
  const [minCount, setMinCount] = useState('1');
  const [maxCount, setMaxCount] = useState('');
  const [poll, setPoll] = useState('300');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const parsed = {
    min: Number.parseFloat(min),
    max: Number.parseFloat(max),
    minCount: Number.parseInt(minCount, 10),
    maxCount: Number.parseInt(maxCount, 10),
    poll: Number.parseInt(poll, 10),
  };
  const ready =
    Number.isFinite(parsed.min) &&
    Number.isFinite(parsed.max) &&
    parsed.min >= 0 &&
    parsed.max > parsed.min &&
    Number.isInteger(parsed.minCount) &&
    parsed.minCount >= 1 &&
    Number.isInteger(parsed.maxCount) &&
    parsed.maxCount >= parsed.minCount &&
    Number.isInteger(parsed.poll) &&
    parsed.poll > 0;

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!ready || busy) return;

    setBusy(true);
    setError(null);

    createRule(serviceId, {
      environment_id: environmentId,
      metric,
      min_threshold: parsed.min,
      max_threshold: parsed.max,
      min_count: parsed.minCount,
      max_count: parsed.maxCount,
      poll_frequency_secs: parsed.poll,
    })
      .then((rule) => {
        setMin('');
        setMax('');
        setMaxCount('');
        onCreated(rule);
      })
      .catch((cause: unknown) => {
        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => setError('Your session has expired. Reload to sign in.'));
          return;
        }

        setError(
          cause instanceof RequestRejected ? cause.message : 'The rule could not be created.',
        );
      })
      .finally(() => setBusy(false));
  };

  return (
    <form className="autoscaling__form" onSubmit={submit}>
      <label className="visually-hidden" htmlFor={`${id}-metric`}>
        Metric
      </label>
      <span className="select-wrap">
        <select
          id={`${id}-metric`}
          className="autoscaling__input autoscaling__input--metric"
          value={metric}
          onChange={(event) => setMetric(event.currentTarget.value as Metric)}
          disabled={busy}
        >
          {(Object.keys(METRICS) as Metric[]).map((option) => (
            <option key={option} value={option}>
              {METRICS[option].label}
            </option>
          ))}
        </select>
      </span>

      <label className="visually-hidden" htmlFor={`${id}-min`}>
        Minimum threshold in {METRICS[metric].unit}
      </label>
      <input
        id={`${id}-min`}
        className="autoscaling__input"
        inputMode="decimal"
        placeholder={`min ${METRICS[metric].unit}`}
        value={min}
        onChange={(event) => setMin(event.currentTarget.value)}
        disabled={busy}
      />

      <label className="visually-hidden" htmlFor={`${id}-max`}>
        Maximum threshold in {METRICS[metric].unit}
      </label>
      <input
        id={`${id}-max`}
        className="autoscaling__input"
        inputMode="decimal"
        placeholder={`max ${METRICS[metric].unit}`}
        value={max}
        onChange={(event) => setMax(event.currentTarget.value)}
        disabled={busy}
      />

      <label className="visually-hidden" htmlFor={`${id}-min-count`}>
        Minimum replica count
      </label>
      <input
        id={`${id}-min-count`}
        className="autoscaling__input"
        inputMode="numeric"
        placeholder="min replicas"
        value={minCount}
        onChange={(event) => setMinCount(event.currentTarget.value)}
        disabled={busy}
      />

      <label className="visually-hidden" htmlFor={`${id}-max-count`}>
        Maximum replica count
      </label>
      <input
        id={`${id}-max-count`}
        className="autoscaling__input"
        inputMode="numeric"
        placeholder="max replicas"
        value={maxCount}
        onChange={(event) => setMaxCount(event.currentTarget.value)}
        disabled={busy}
      />

      <label className="visually-hidden" htmlFor={`${id}-poll`}>
        Poll frequency in seconds
      </label>
      <input
        id={`${id}-poll`}
        className="autoscaling__input"
        inputMode="numeric"
        placeholder="every (s)"
        value={poll}
        onChange={(event) => setPoll(event.currentTarget.value)}
        disabled={busy}
      />

      <button className="autoscaling__submit" type="submit" disabled={busy || !ready}>
        {busy ? 'Adding…' : 'Add rule'}
      </button>

      {error && (
        <p className="notice notice--error autoscaling__error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
