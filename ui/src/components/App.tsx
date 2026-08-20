import LoginButton from './LoginButton';
import ProjectList from './ProjectList';
import UserMenu from './UserMenu';
import { SessionProvider, useSession } from '../lib/session';

/**
 * The page, under one session.
 *
 * The account bar, the login button and the projects are three answers to the
 * same question, so they read it from one provider rather than asking
 * separately. React context does not cross Astro island boundaries, which is
 * why this is a single island rather than one per component.
 */
export default function App() {
  return (
    <SessionProvider>
      <UserMenu />
      <main>
        <Home />
      </main>
    </SessionProvider>
  );
}

/**
 * What the page is for, which depends on whether there is a login.
 *
 * Neither branch renders until the session is known: an invitation to sign in,
 * shown to someone who already has, is worse than a moment of nothing.
 */
function Home() {
  const session = useSession();

  if (session.isSignedIn()) {
    return (
      <>
        <h1>monorail</h1>
        <h2 className="section-heading">Projects</h2>
        <ProjectList />
      </>
    );
  }

  if (session.isSignedOut()) {
    return (
      <div className="hero">
        <h1>monorail</h1>
        <p>Sign in with your Railway account to continue.</p>
        <LoginButton />
      </div>
    );
  }

  return null;
}
