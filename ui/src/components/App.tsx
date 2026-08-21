import { useId } from 'react';

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
      <Home />
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
        <header className="site-header">
          <div className="site-header__inner">
            <span className="brand">
              <Mark className="brand__mark" />
              monorail
            </span>
            <UserMenu />
          </div>
        </header>
        <main>
          <div className="page-head">
            <h1 className="page-title">Projects</h1>
            <p className="page-subtitle">
              Everything on your Railway account — services, deployments and autoscaling.
            </p>
          </div>
          <ProjectList />
        </main>
      </>
    );
  }

  if (session.isSignedOut()) {
    return (
      <main>
        <div className="hero">
          <Mark className="hero__mark" />
          <h1 className="hero__title">monorail</h1>
          <p className="hero__tagline">
            Your Railway projects on one track — inspect every service, spin deployments up and
            down, and autoscale them on live metrics.
          </p>
          <LoginButton />
          <p className="hero__footnote">Connects to your Railway account over OAuth.</p>
        </div>
      </main>
    );
  }

  return null;
}

/**
 * The mark: a monorail car on its rail. The gradient id is minted per instance
 * because SVG ids are document-global, and this renders in more than one place.
 */
function Mark({ className }: { className: string }) {
  const id = useId();

  return (
    <svg className={className} viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#8b5cf6" />
          <stop offset="1" stopColor="#6366f1" />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="8" fill={`url(#${id})`} />
      <rect x="8" y="9" width="16" height="8" rx="4" fill="#fff" />
      <path d="M5.5 21.5h21" stroke="#fff" strokeWidth="2.5" strokeLinecap="round" />
    </svg>
  );
}
