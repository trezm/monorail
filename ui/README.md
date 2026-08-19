# ui

The web front end: [Astro](https://astro.build) with React islands.

```bash
bazel run //ui:dev     # http://localhost:4321
bazel build //ui       # static site into bazel-bin/ui/dist
```

One page so far — a Login with Railway button.

## Layout

| Path | |
|---|---|
| `src/pages/index.astro` | The only route. |
| `src/components/LoginButton.tsx` | The button, hydrated with `client:load`. |
| `src/styles/global.css` | Everything visual. No framework. |
| `astro.config.mjs` | React integration, and the output paths Bazel overrides. |

The button is an anchor, not a `<button>` with an `onClick`. An OAuth redirect
has to be a top-level navigation, so `fetch` cannot start one, and a real link
keeps middle-click, keyboard activation and the status bar working. It is a
React component anyway because the redirect takes a moment and the click needs
acknowledging.

Reading the logged-in user back from `GET /api/v1/auth/me` is the obvious next
thing; the API already serves it, and CORS already allows credentials from this
origin.

## Configuration

`PUBLIC_API_URL`, defaulting to `http://localhost:8080`. `PUBLIC_` is Astro's
prefix for values that reach the browser, which this one must: the button is a
link to the API, so the URL ends up in the page. See [`.env.example`](.env.example).

The API needs to allow this origin to send its session cookie:

```bash
API_CORS_ALLOWED_ORIGINS=http://localhost:4321
```

A wildcard will not do — the CORS specification forbids pairing credentials with
one.

## Dependencies

`pnpm-lock.yaml` at the repo root is the source of truth, the way `Cargo.lock`
is for Rust, and `rules_js` mirrors it into Bazel. Add a dependency with:

```bash
pnpm --filter ui add <package>
```

Bazel picks up the rewritten lockfile on the next build; there is no repin step.

`pnpm install` at the root is only needed for the editor and for running
`astro` outside Bazel — the build materialises its own `node_modules`.
