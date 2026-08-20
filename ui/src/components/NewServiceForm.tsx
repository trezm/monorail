import { useId, useState, type FormEvent } from 'react';

import {
  createService,
  RequestRejected,
  SessionExpired,
  type NewServiceSource,
  type Service,
} from '../lib/projects';
import { useSession } from '../lib/session';

type SourceKind = 'docker_image' | 'github_repo';

const SOURCE_KINDS: Record<SourceKind, { label: string; placeholder: string }> = {
  docker_image: { label: 'Docker image', placeholder: 'nginx:latest' },
  github_repo: { label: 'GitHub repo', placeholder: 'owner/repo' },
};

/**
 * Adds a service to one project, from a Docker image or a GitHub repo — the
 * only two sources the API accepts, so the whole form is one choice and one
 * value. Railway names the service and owns every other setting.
 */
export default function NewServiceForm({
  projectId,
  onCreated,
}: {
  projectId: string;
  onCreated: (service: Service) => void;
}) {
  const session = useSession();
  const id = useId();
  const [kind, setKind] = useState<SourceKind>('docker_image');
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();

    const trimmed = value.trim();
    if (!trimmed || busy) return;

    const source: NewServiceSource =
      kind === 'docker_image' ? { docker_image: trimmed } : { github_repo: repoSlug(trimmed) };

    setBusy(true);
    setError(null);

    createService(projectId, source)
      .then((service) => {
        setValue('');
        onCreated(service);
      })
      .catch((cause: unknown) => {
        // Same as loading the list: a spent Railway token only a new login can
        // fix means the session is over, and ending it shows the login button.
        if (cause instanceof SessionExpired) {
          session.logOut().catch(() => setError('Your session has expired. Reload to sign in.'));
          return;
        }

        setError(
          cause instanceof RequestRejected ? cause.message : 'The service could not be created.',
        );
      })
      .finally(() => setBusy(false));
  };

  return (
    <form className="new-service" onSubmit={submit}>
      <fieldset className="new-service__kinds">
        <legend className="visually-hidden">New service source</legend>
        {(Object.keys(SOURCE_KINDS) as SourceKind[]).map((option) => (
          <label key={option} className="new-service__kind">
            <input
              type="radio"
              name={`${id}-kind`}
              checked={kind === option}
              onChange={() => setKind(option)}
              disabled={busy}
            />
            {SOURCE_KINDS[option].label}
          </label>
        ))}
      </fieldset>

      <div className="new-service__row">
        <label className="visually-hidden" htmlFor={`${id}-value`}>
          {SOURCE_KINDS[kind].label}
        </label>
        <input
          id={`${id}-value`}
          className="new-service__input"
          value={value}
          placeholder={SOURCE_KINDS[kind].placeholder}
          onChange={(event) => setValue(event.currentTarget.value)}
          disabled={busy}
        />
        <button className="new-service__submit" type="submit" disabled={busy || !value.trim()}>
          {busy ? 'Adding…' : 'Add service'}
        </button>
      </div>

      {error && (
        <p className="notice notice--error new-service__error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/**
 * Railway takes repos as `owner/repo`, but a pasted GitHub URL is the likelier
 * input — reduce one to the other rather than bounce it.
 */
function repoSlug(value: string): string {
  return value
    .replace(/^https?:\/\/(www\.)?github\.com\//, '')
    .replace(/\.git$/, '')
    .replace(/\/+$/, '');
}
