import { useState } from 'react';

// Only reached by a plain `pnpm dev` with no `.env`. Every Bazel build is
// given the value, and one given an empty value fails in astro.config.mjs
// rather than shipping this.
const API_URL = import.meta.env.PUBLIC_API_URL ?? 'http://localhost:8080';

/**
 * Starts the Railway login.
 *
 * An anchor rather than a button with an onClick handler: the OAuth redirect
 * has to be a top-level navigation, so `fetch` cannot start it and a real link
 * keeps middle-click, keyboard activation and the status bar working. The
 * component is interactive only to acknowledge the click, since the redirect
 * itself takes a moment.
 */
export default function LoginButton() {
  const [redirecting, setRedirecting] = useState(false);

  return (
    <a
      className="login-button"
      href={`${API_URL}/auth/railway`}
      aria-busy={redirecting}
      onClick={() => setRedirecting(true)}
    >
      <svg className="login-button__mark" viewBox="0 0 24 24" aria-hidden="true">
        <path
          fill="currentColor"
          d="M1.3 9.9h13.4c.3 0 .4-.4.1-.5A9.6 9.6 0 0 1 10.6 6a.5.5 0 0 0-.4-.2H.6a.4.4 0 0 0-.4.3 12 12 0 0 0-.2 3.4c0 .2.2.4.4.4Zm22.4 4.2H1.6a.4.4 0 0 0-.4.3 12 12 0 0 0 1 2.3c0 .2.2.2.4.2h20.4c.2 0 .4-.1.4-.3a12 12 0 0 0 .5-2.1.4.4 0 0 0-.2-.4ZM3.8 5.2h5.3c.3 0 .4-.4.2-.6a9.6 9.6 0 0 1-1.5-1.9.4.4 0 0 0-.5-.2 12 12 0 0 0-3.7 2.1c-.2.2-.1.6.2.6Zm19.5 4.7H17c-.2 0-.4.2-.4.4v3.1c0 .2.2.4.4.4h6.4c.2 0 .4-.2.4-.4V10.3c0-.2-.2-.4-.4-.4ZM3 18.8a12 12 0 0 0 18 0 .4.4 0 0 0-.3-.6H3.3a.4.4 0 0 0-.3.6Z"
        />
      </svg>
      {redirecting ? 'Redirecting…' : 'Login with Railway'}
    </a>
  );
}
