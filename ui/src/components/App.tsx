import LoginButton from './LoginButton';
import UserMenu from './UserMenu';
import { SessionProvider } from '../lib/session';

/**
 * The page, under one session.
 *
 * The account bar and the login button are two answers to the same question, so
 * they read it from one provider rather than asking separately. React context
 * does not cross Astro island boundaries, which is why this is a single island
 * rather than one per component.
 */
export default function App() {
  return (
    <SessionProvider>
      <UserMenu />
      <main>
        <h1>monorail</h1>
        <p>Sign in with your Railway account to continue.</p>
        <LoginButton />
      </main>
    </SessionProvider>
  );
}
