import { useEffect, useState } from 'react';

import {
  environments as fetchEnvironments,
  serviceInstance,
  SessionExpired,
  type Environment,
  type Project,
  type ServiceInstance,
} from '../lib/projects';
import { useSession } from '../lib/session';

/**
 * One project's services, detailed for one of its environments at a time.
 *
 * Environments load the first time the project is expanded — `active` going
 * true is that signal — rather than for every project on page load, and stay
 * loaded across collapses. Picking one in the dropdown refetches each
 * service's instance for it: an instance is keyed by service and environment
 * together, and a service does not have to be deployed in every environment.
 */
export default function ProjectServices({ project, active }: { project: Project; active: boolean }) {
  const session = useSession();
  const [started, setStarted] = useState(active);
  const [loaded, setLoaded] = useState<Environment[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [instances, setInstances] = useState<Record<string, InstanceState>>({});

  useEffect(() => {
    if (active) setStarted(true);
  }, [active]);

  useEffect(() => {
    if (!started) return undefined;

    let live = true;

    fetchEnvironments(project.id)
      .then((found) => {
        if (!live) return;

        setLoaded(found);

        // Production is the environment the dashboard is usually opened for;
        // anything else is a deliberate pick from the dropdown.
        const initial = found.find((environment) => environment.name === 'production') ?? found[0];
        setSelected(initial ? initial.id : null);
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
  }, [started, project.id, session]);

  useEffect(() => {
    if (selected === null) return undefined;

    let live = true;

    setInstances(
      Object.fromEntries(
        project.services.map((service): [string, InstanceState] => [
          service.id,
          { status: 'loading' },
        ]),
      ),
    );

    for (const service of project.services) {
      serviceInstance(service.id, selected)
        .then((instance) => {
          if (!live) return;

          setInstances((current) => ({
            ...current,
            [service.id]: { status: 'loaded', instance },
          }));
        })
        .catch((cause: unknown) => {
          if (!live) return;

          if (cause instanceof SessionExpired) {
            session.logOut().catch(() => {});
          }

          setInstances((current) => ({ ...current, [service.id]: { status: 'failed' } }));
        });
    }

    return () => {
      live = false;
    };
  }, [selected, project.services, session]);

  if (failed) {
    return <p className="project__empty notice--error">The environments could not be loaded.</p>;
  }
  if (loaded === null) return <p className="project__empty">Loading environments…</p>;

  return (
    <>
      {loaded.length > 0 && (
        <label className="environments">
          <span className="environments__label">Environment</span>
          <select
            className="environments__select"
            value={selected ?? ''}
            onChange={(event) => setSelected(event.target.value)}
          >
            {loaded.map((environment) => (
              <option key={environment.id} value={environment.id}>
                {environment.name}
              </option>
            ))}
          </select>
        </label>
      )}

      <ul className="services">
        {project.services.map((service) => (
          <li key={service.id} className="service">
            <div className="service__header">
              <span className="service__name">{service.name}</span>
              {service.created_at && (
                <time className="service__created" dateTime={service.created_at}>
                  {formatDate(service.created_at)}
                </time>
              )}
            </div>
            {selected !== null && (
              <InstanceDetails state={instances[service.id] ?? { status: 'loading' }} />
            )}
          </li>
        ))}
      </ul>
    </>
  );
}

type InstanceState =
  | { status: 'loading' }
  | { status: 'failed' }
  | { status: 'loaded'; instance: ServiceInstance | null };

function InstanceDetails({ state }: { state: InstanceState }) {
  if (state.status === 'loading') return <p className="service__note">Loading details…</p>;
  if (state.status === 'failed') {
    return <p className="service__note service__note--error">The details could not be loaded.</p>;
  }
  if (state.instance === null) {
    return <p className="service__note">Not deployed in this environment.</p>;
  }

  const instance = state.instance;
  const deployment = instance.latest_deployment;

  return (
    <dl className="instance">
      <dt className="instance__term">Deployment</dt>
      <dd className="instance__value">
        {deployment ? (
          <>
            <span className={statusClass(deployment.status)}>{words(deployment.status)}</span>
            {deployment.created_at && (
              <>
                {' · '}
                <time dateTime={deployment.created_at}>{formatDate(deployment.created_at)}</time>
              </>
            )}
          </>
        ) : (
          'none yet'
        )}
      </dd>
      {instance.region && <Field term="Region" value={instance.region} />}
      {instance.num_replicas !== null && (
        <Field
          term="Replicas"
          value={String(instance.num_replicas)}
        />
      )}
      {instance.restart_policy_type && (
        <Field
          term="Restart policy"
          value={restartPolicy(instance.restart_policy_type, instance.restart_policy_max_retries)}
        />
      )}
      {instance.root_directory && <Field term="Root directory" value={instance.root_directory} code />}
      {instance.build_command && <Field term="Build command" value={instance.build_command} code />}
      {instance.start_command && <Field term="Start command" value={instance.start_command} code />}
      {instance.healthcheck_path && (
        <Field term="Healthcheck" value={instance.healthcheck_path} code />
      )}
    </dl>
  );
}

function Field({ term, value, code = false }: { term: string; value: string; code?: boolean }) {
  return (
    <>
      <dt className="instance__term">{term}</dt>
      <dd className={code ? 'instance__value instance__value--code' : 'instance__value'}>
        {value}
      </dd>
    </>
  );
}

/** Railway enums arrive as `SCREAMING_SNAKE`; people read lowercase words. */
function words(value: string) {
  return value.toLowerCase().replaceAll('_', ' ');
}

function restartPolicy(type: string, maxRetries: number | null) {
  return type === 'ON_FAILURE' && maxRetries !== null && maxRetries > 0
    ? `${words(type)} · up to ${maxRetries} ${maxRetries === 1 ? 'retry' : 'retries'}`
    : words(type);
}

function statusClass(status: string) {
  if (status === 'SUCCESS') return 'instance__status instance__status--ok';
  if (status === 'CRASHED' || status === 'FAILED') return 'instance__status instance__status--bad';

  return 'instance__status';
}

/** Falls back to the raw value rather than rendering `Invalid Date`. */
function formatDate(value: string) {
  const parsed = new Date(value);

  return Number.isNaN(parsed.valueOf())
    ? value
    : parsed.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}
