/** The signed-in user, as the browser sees it. */

import { useEffect, useState } from 'react';

// Only reached by a plain `pnpm dev` with no `.env`. Every Bazel build is
// given the value, and one given an empty value fails in astro.config.mjs
// rather than shipping this.
export const API_URL = import.meta.env.PUBLIC_API_URL ?? 'http://localhost:8080';

/** The `GET /api/v1/users/me` body. Mirrors `Profile` in `api/src/routes/users.rs`. */
export interface Profile {
  id: string;
  email: string | null;
  name: string | null;
  avatar_url: string | null;
}

export type Session =
  | { status: 'loading' }
  | { status: 'signed-in'; user: Profile }
  | { status: 'signed-out' };

let pending: Promise<Profile | null> | undefined;

/**
 * Whether anyone is signed in, resolved in the browser.
 *
 * The site is static, so the build knows nothing about the session and every
 * island has to ask. The request is shared through a module-level promise so
 * several islands mounting together still cost one call; a rejection is not
 * cached, since the only way back from a network failure is to try again.
 *
 * A failure is reported as signed out. The distinction between "no session"
 * and "could not tell" would only ever be acted on the same way.
 */
export function useSession(): Session {
  const [session, setSession] = useState<Session>({ status: 'loading' });

  useEffect(() => {
    let live = true;

    profile()
      .then((user) => {
        if (live) {
          setSession(user ? { status: 'signed-in', user } : { status: 'signed-out' });
        }
      })
      .catch(() => {
        if (live) setSession({ status: 'signed-out' });
      });

    return () => {
      live = false;
    };
  }, []);

  return session;
}

/** Ends the session server-side, which is what clears the cookie. */
export async function endSession(): Promise<void> {
  const response = await fetch(`${API_URL}/auth/session`, {
    method: 'DELETE',
    credentials: 'include',
  });

  if (!response.ok) {
    throw new Error(`DELETE /auth/session answered ${response.status}`);
  }
}

function profile(): Promise<Profile | null> {
  pending ??= request().catch((error: unknown) => {
    pending = undefined;
    throw error;
  });

  return pending;
}

// `credentials: 'include'` is what attaches the session cookie across origins;
// the API allows it for this one. 401 is the ordinary answer for a visitor
// who has not logged in, so it is not an error.
async function request(): Promise<Profile | null> {
  const response = await fetch(`${API_URL}/api/v1/users/me`, {
    credentials: 'include',
  });

  if (response.status === 401) return null;

  if (!response.ok) {
    throw new Error(`GET /api/v1/users/me answered ${response.status}`);
  }

  return (await response.json()) as Profile;
}
