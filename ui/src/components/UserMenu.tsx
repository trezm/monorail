import { useState } from 'react';

import { endSession, useSession } from '../lib/session';

/**
 * Who is signed in, and the way out.
 *
 * Renders nothing until the session is known, and nothing at all without one:
 * a bar that appears empty and then fills in reads worse than one that arrives
 * with something to say. Logging out reloads rather than clearing local state,
 * so every island on the page re-reads the session from the API instead of
 * being told about it.
 */
export default function UserMenu() {
  const session = useSession();
  const [leaving, setLeaving] = useState(false);

  if (session.status !== 'signed-in') return null;

  const { user } = session;

  async function logout() {
    setLeaving(true);

    try {
      await endSession();
      window.location.reload();
    } catch {
      setLeaving(false);
    }
  }

  return (
    <nav className="user-menu" aria-label="Account">
      {user.avatar_url && (
        <img className="user-menu__avatar" src={user.avatar_url} alt="" width={28} height={28} />
      )}
      <span className="user-menu__name">{user.name ?? user.email ?? 'Signed in'}</span>
      <button className="user-menu__logout" type="button" onClick={logout} disabled={leaving}>
        {leaving ? 'Logging out…' : 'Log out'}
      </button>
    </nav>
  );
}
