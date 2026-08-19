import { useState } from 'react';

import { useSession } from '../lib/session';

/**
 * Who is signed in, and the way out.
 *
 * Renders nothing until the session is known, and nothing at all without one:
 * a bar that appears empty and then fills in reads worse than one that arrives
 * with something to say. Logging out updates the provider rather than reloading
 * the page, so the login button takes its place without a round trip.
 */
export default function UserMenu() {
  const session = useSession();
  const { user } = session;
  const [leaving, setLeaving] = useState(false);

  const leave = async () => {
    setLeaving(true);

    try {
      await session.logOut();
    } catch {
      setLeaving(false);
    }
  };

  if (!session.isSignedIn() || user === null) return null;

  return (
    <nav className="user-menu" aria-label="Account">
      {user.avatar_url && (
        <img className="user-menu__avatar" src={user.avatar_url} alt="" width={28} height={28} />
      )}
      <span className="user-menu__name">{user.name ?? user.email ?? 'Signed in'}</span>
      <button className="user-menu__logout" type="button" onClick={leave} disabled={leaving}>
        {leaving ? 'Logging out…' : 'Log out'}
      </button>
    </nav>
  );
}
