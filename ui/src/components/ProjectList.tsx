import { useEffect, useState } from 'react';

import { ApiError, getProjects, type Project } from '../lib/api';

/**
 * The projects on the account, each an expandable row over its services.
 *
 * A native `<details>` rather than a hand-rolled disclosure: it is keyboard
 * operable, announced as expandable, and findable by the browser's own
 * in-page search before any of this code runs. `open` is still controlled, so
 * an account with a single project can start expanded without React and the
 * DOM disagreeing about the attribute afterwards.
 */
export default function ProjectList({ onUnauthorized }: { onUnauthorized: () => void }) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());

  useEffect(() => {
    let live = true;

    getProjects()
      .then((loaded) => {
        if (!live) return;

        setProjects(loaded);
        setExpanded(new Set(loaded.length === 1 ? [loaded[0].id] : []));
      })
      .catch((cause: unknown) => {
        if (!live) return;

        // The session cookie is fine; the Railway token behind it is spent, and
        // only a new login replaces it.
        if (cause instanceof ApiError && cause.isUnauthorized) {
          onUnauthorized();
          return;
        }

        setError(cause instanceof ApiError ? cause.message : 'the projects could not be loaded');
      });

    return () => {
      live = false;
    };
  }, [onUnauthorized]);

  function toggle(id: string, open: boolean) {
    setExpanded((current) => {
      const next = new Set(current);
      if (open) next.add(id);
      else next.delete(id);

      return next;
    });
  }

  if (error) {
    return <p className="notice notice--error">{error}</p>;
  }

  if (!projects) {
    return <p className="notice">Loading your projects…</p>;
  }

  if (projects.length === 0) {
    return <p className="notice">This Railway account has no projects yet.</p>;
  }

  return (
    <ul className="projects">
      {projects.map((project) => (
        <li key={project.id}>
          <details
            className="project"
            open={expanded.has(project.id)}
            onToggle={(event) => toggle(project.id, event.currentTarget.open)}
          >
            <summary className="project__summary">
              <svg className="project__chevron" viewBox="0 0 16 16" aria-hidden="true">
                <path
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="m6 3 5 5-5 5"
                />
              </svg>
              <span className="project__name">{project.name}</span>
              <span className="project__count">
                {project.services.length}{' '}
                {project.services.length === 1 ? 'service' : 'services'}
              </span>
            </summary>

            {project.description && <p className="project__description">{project.description}</p>}

            {project.services.length === 0 ? (
              <p className="project__empty">No services in this project.</p>
            ) : (
              <ul className="services">
                {project.services.map((service) => (
                  <li key={service.id} className="service">
                    <span className="service__name">{service.name}</span>
                    {service.created_at && (
                      <time className="service__created" dateTime={service.created_at}>
                        {formatDate(service.created_at)}
                      </time>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </details>
        </li>
      ))}
    </ul>
  );
}

/** Falls back to the raw value rather than rendering `Invalid Date`. */
function formatDate(value: string) {
  const parsed = new Date(value);

  return Number.isNaN(parsed.valueOf())
    ? value
    : parsed.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}
