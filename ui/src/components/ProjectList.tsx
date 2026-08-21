import { useEffect, useState } from 'react';

import NewServiceForm from './NewServiceForm';
import ProjectServices from './ProjectServices';
import { projects, SessionExpired, type Project, type Service } from '../lib/projects';
import { useSession } from '../lib/session';

/**
 * The projects on the account, each an expandable row over its services.
 *
 * A native `<details>` rather than a hand-rolled disclosure: it is keyboard
 * operable, announced as expandable, and findable by the browser's own in-page
 * search before any of this code runs. `open` is still controlled, so an
 * account with a single project can start expanded without React and the DOM
 * disagreeing about the attribute afterwards.
 */
export default function ProjectList() {
  const session = useSession();
  const [loaded, setLoaded] = useState<Project[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [refreshes, setRefreshes] = useState<Readonly<Record<string, 'busy' | 'failed'>>>({});

  useEffect(() => {
    let live = true;

    projects()
      .then((found) => {
        if (!live) return;

        setLoaded(found);
        setExpanded(new Set(found.length === 1 ? [found[0].id] : []));
      })
      .catch((cause: unknown) => {
        if (!live) return;

        // Nothing local can renew a spent Railway token, so the session is over
        // and the provider should say so — that puts the login button back.
        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => setFailed(true));
          return;
        }

        setFailed(true);
      });

    return () => {
      live = false;
    };
  }, [session]);

  // Inserted in name order, where a fresh GET would have put it.
  const addService = (projectId: string, service: Service) => {
    setLoaded((current) => {
      if (!current) return current;

      return current.map((project) =>
        project.id === projectId
          ? {
              ...project,
              services: [...project.services, service].sort((a, b) =>
                a.name.localeCompare(b.name),
              ),
            }
          : project,
      );
    });
  };

  /**
   * Refetches one project's row. The API only serves the whole list — Railway
   * assembles it in a single query anyway, so a per-project GET would be the
   * same round trip — and the fresh copy replaces just the asked-about
   * project, or drops it when the account no longer has it. A new `services`
   * array is what makes `ProjectServices` refetch its instance details.
   */
  const refresh = (projectId: string) => {
    setRefreshes((current) => ({ ...current, [projectId]: 'busy' }));

    projects()
      .then((found) => {
        const fresh = found.find((project) => project.id === projectId);

        setLoaded((current) => {
          if (!current) return current;

          return fresh
            ? current.map((project) => (project.id === projectId ? fresh : project))
            : current.filter((project) => project.id !== projectId);
        });
        setRefreshes(({ [projectId]: done, ...rest }) => rest);
      })
      .catch((cause: unknown) => {
        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => setFailed(true));
          return;
        }

        setRefreshes((current) => ({ ...current, [projectId]: 'failed' }));
      });
  };

  const toggle = (id: string, open: boolean) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (open) next.add(id);
      else next.delete(id);

      return next;
    });
  };

  if (failed) return <p className="notice notice--error">Your projects could not be loaded.</p>;

  if (loaded === null) {
    return (
      <div className="skeleton-list" role="status">
        <span className="visually-hidden">Loading your projects…</span>
        {[0, 1, 2].map((row) => (
          <div key={row} className="skeleton-card" aria-hidden="true">
            <span className="skeleton skeleton--title" />
            <span className="skeleton skeleton--pill" />
          </div>
        ))}
      </div>
    );
  }

  if (loaded.length === 0) {
    return (
      <div className="empty-state">
        <svg
          className="empty-state__icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m12 3 9 5-9 5-9-5 9-5Z" />
          <path d="m3 13.5 9 5 9-5" />
        </svg>
        <p className="empty-state__title">No projects yet</p>
        <p className="empty-state__hint">Projects you create on Railway will appear here.</p>
      </div>
    );
  }

  return (
    <ul className="projects">
      {loaded.map((project) => (
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
                {project.services.length} {project.services.length === 1 ? 'service' : 'services'}
              </span>
              <button
                type="button"
                className={
                  refreshes[project.id] === 'busy'
                    ? 'project__refresh project__refresh--busy'
                    : 'project__refresh'
                }
                aria-label={`Refresh ${project.name}`}
                title="Refresh"
                disabled={refreshes[project.id] === 'busy'}
                onClick={(event) => {
                  // Activating a button inside <summary> still toggles the
                  // disclosure unless the default is prevented.
                  event.preventDefault();
                  refresh(project.id);
                }}
              >
                <svg
                  className="project__refresh-icon"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2.6"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                >
                  <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
                  <path d="M21 3v5h-5" />
                </svg>
              </button>
            </summary>

            {project.description && <p className="project__description">{project.description}</p>}

            {project.services.length === 0 ? (
              <p className="project__empty">No services in this project.</p>
            ) : (
              <ProjectServices project={project} active={expanded.has(project.id)} />
            )}

            <NewServiceForm
              projectId={project.id}
              onCreated={(service) => addService(project.id, service)}
            />
          </details>
          {refreshes[project.id] === 'failed' && (
            <p className="project__refresh-error" role="alert">
              {project.name} could not be refreshed.
            </p>
          )}
        </li>
      ))}
    </ul>
  );
}
