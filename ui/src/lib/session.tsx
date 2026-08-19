/** The signed-in user, as the browser sees it. */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

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

export enum SessionStatus {
  Loading = 'loading',
  SignedIn = 'signed-in',
  SignedOut = 'signed-out',
}

/**
 * Who is signed in, and how to stop being.
 *
 * `isSignedIn` and `isSignedOut` are not each other's negation: until the first
 * request answers, both are false. That third state is the point — rendering
 * either the account bar or a login button before the answer arrives shows one
 * of them to the wrong person.
 */
export interface Session {
  status: SessionStatus;
  user: Profile | null;
  isSignedIn(): boolean;
  isSignedOut(): boolean;
  logOut(): Promise<void>;
}

const SessionContext = createContext<Session | null>(null);

/**
 * Resolves the session once and hands it to everything below.
 *
 * The site is static, so the build knows nothing about who is looking at it and
 * the browser has to ask. One provider per React root, which on an Astro page
 * means one island: context does not cross island boundaries, so the components
 * that need the session are mounted together underneath this.
 */
export function SessionProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState(SessionStatus.Loading);
  const [user, setUser] = useState<Profile | null>(null);

  // The cookie is HttpOnly, so the server ending the session is the only thing
  // that can clear it. Nothing local is authoritative until it answers.
  const logOut = useCallback(async () => {
    const response = await fetch(`${API_URL}/auth/session`, {
      method: 'DELETE',
      credentials: 'include',
    });

    if (!response.ok) {
      throw new Error(`DELETE /auth/session answered ${response.status}`);
    }

    setUser(null);
    setStatus(SessionStatus.SignedOut);
  }, []);

  const session = useMemo<Session>(
    () => ({
      status,
      user,
      isSignedIn: () => status === SessionStatus.SignedIn,
      isSignedOut: () => status === SessionStatus.SignedOut,
      logOut,
    }),
    [status, user, logOut],
  );

  useEffect(() => {
    let live = true;

    profile()
      .then((found) => {
        if (!live) return;

        setUser(found);
        setStatus(found ? SessionStatus.SignedIn : SessionStatus.SignedOut);
      })
      .catch(() => {
        if (live) setStatus(SessionStatus.SignedOut);
      });

    return () => {
      live = false;
    };
  }, []);

  return <SessionContext.Provider value={session}>{children}</SessionContext.Provider>;
}

export function useSession(): Session {
  const session = useContext(SessionContext);

  if (!session) {
    throw new Error('useSession is only usable inside a SessionProvider');
  }

  return session;
}

/**
 * A failure is reported as no session. The distinction between "not signed in"
 * and "could not tell" would only ever be acted on the same way.
 */
async function profile(): Promise<Profile | null> {
  const response = await fetch(`${API_URL}/api/v1/users/me`, {
    credentials: 'include',
  });

  // `credentials: 'include'` is what attaches the session cookie across
  // origins; the API allows it for this one. 401 is the ordinary answer for a
  // visitor who has not logged in, so it is not an error.
  if (response.status === 401) return null;

  if (!response.ok) {
    throw new Error(`GET /api/v1/users/me answered ${response.status}`);
  }

  return (await response.json()) as Profile;
}
