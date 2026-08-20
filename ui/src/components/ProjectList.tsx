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

  const toggle = (id: string, open: boolean) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (open) next.add(id);
      else next.delete(id);

      return next;
    });
  };

  if (failed) return <p className="notice notice--error">Your projects could not be loaded.</p>;
  if (loaded === null) return <p className="notice">Loading your projects…</p>;
  if (loaded.length === 0) return <p className="notice">This Railway account has no projects yet.</p>;

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
        </li>
      ))}
    </ul>
  );
}
