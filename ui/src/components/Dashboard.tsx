import { useCallback, useEffect, useState } from 'react';

import LoginButton from './LoginButton';
import ProjectList from './ProjectList';
import { ApiError, getProfile, logOut, type Profile } from '../lib/api';

type State =
  | { kind: 'checking' }
  | { kind: 'signed-out'; error?: string }
  | { kind: 'signed-in'; profile: Profile };

/**
 * The whole page: whether there is a login, and what to show either way.
 *
 * The site is static, so who is signed in cannot be known at build time and is
 * asked for on mount instead. The `checking` state exists so a signed-in
 * visitor is never shown a login button that is about to disappear.
 */
export default function Dashboard() {
  const [state, setState] = useState<State>({ kind: 'checking' });

  const signOut = useCallback(() => {
    setState({ kind: 'signed-out' });
  }, []);

  useEffect(() => {
    let live = true;

    getProfile()
      .then((profile) => live && setState({ kind: 'signed-in', profile }))
      .catch((cause: unknown) => {
        if (!live) return;

        setState({
          kind: 'signed-out',
          error:
            cause instanceof ApiError && !cause.isUnauthorized ? cause.message : undefined,
        });
      });

    return () => {
      live = false;
    };
  }, []);

  if (state.kind === 'checking') {
    return <p className="notice">Checking your session…</p>;
  }

  if (state.kind === 'signed-out') {
    return (
      <div className="hero">
        <h1>monorail</h1>
        <p>Sign in with your Railway account to continue.</p>
        {state.error && <p className="notice notice--error">{state.error}</p>}
        <LoginButton />
      </div>
    );
  }

  return (
    <>
      <header className="masthead">
        <div className="masthead__identity">
          <h1>monorail</h1>
          <p className="masthead__user">Signed in as {displayName(state.profile)}</p>
        </div>
        <SignOutButton onSignedOut={signOut} />
      </header>

      <h2 className="section-heading">Projects</h2>
      <ProjectList onUnauthorized={signOut} />
    </>
  );
}

/**
 * A button and not a link: signing out is a `DELETE`, and the page it lands on
 * is the one already open.
 */
function SignOutButton({ onSignedOut }: { onSignedOut: () => void }) {
  const [pending, setPending] = useState(false);

  return (
    <button
      type="button"
      className="text-button"
      disabled={pending}
      onClick={() => {
        setPending(true);
        // A failed logout still ends the session as far as this tab is
        // concerned; the cookie either went or was never valid.
        logOut()
          .catch(() => undefined)
          .then(onSignedOut);
      }}
    >
      {pending ? 'Signing out…' : 'Sign out'}
    </button>
  );
}

/** An email is not guaranteed and a name even less so; the id always is. */
function displayName(profile: Profile) {
  return profile.name ?? profile.email ?? profile.id;
}
