# ui

The web front end: [Astro](https://astro.build) with React islands.

```bash
bazel run //ui:dev     # http://localhost:4321
bazel build //ui       # static site into bazel-bin/ui/dist
```

One page so far — a Login with Railway button, and an account bar for whoever
is signed in.

## Layout

| Path | |
|---|---|
| `src/pages/index.astro` | The only route. |
| `src/components/App.tsx` | The one island, hydrated with `client:load`. |
| `src/components/LoginButton.tsx` | The button. |
| `src/components/UserMenu.tsx` | Name, avatar and log out, top right. |
| `src/lib/session.tsx` | The session context, provider and hook. |
| `src/styles/global.css` | Everything visual. No framework. |
| `astro.config.mjs` | React integration, and the output paths Bazel overrides. |

The button is an anchor, not a `<button>` with an `onClick`. An OAuth redirect
has to be a top-level navigation, so `fetch` cannot start one, and a real link
keeps middle-click, keyboard activation and the status bar working. It is a
React component anyway because the redirect takes a moment and the click needs
acknowledging.

## The session

The site is static, so the build knows nothing about who is looking at it and
the browser has to ask. `SessionProvider` requests `GET /api/v1/users/me` once
with `credentials: 'include'`, which is what attaches the session cookie across
the two origins. A `401` is the ordinary answer for a visitor who has not logged
in, not an error.

That answer decides what renders. `session.isSignedIn()` and
`session.isSignedOut()` are not each other's negation — until the request
answers, both are false — so the account bar and the login button each wait for
their own answer instead of one of them showing to the wrong person.

React context does not cross Astro island boundaries, so everything that reads
the session lives under a single `client:load` island rather than one per
component. That is what `App.tsx` is for, and it is why logging out needs no
page reload: `DELETE /auth/session` clears the `HttpOnly` cookie server-side —
nothing else can — and the provider then tells both components at once.

## Configuration

`PUBLIC_API_URL` is where the login button points. `PUBLIC_` is Astro's prefix
for values that reach the browser, which this one must: the button is a link to
the API, so the URL is baked into the page at build time rather than read at
runtime.

How it is supplied depends on how you build:

| | |
|---|---|
| `bazel build //ui` | `--define=PUBLIC_API_URL=...`, defaulting to the local API in [`//.bazelrc`](../.bazelrc) |
| `bazel build --config=release //ui` | the same `--define`, with no default — the build fails without it |
| `pnpm dev` / `pnpm build` | [`ui/.env`](.env.example), which Astro loads itself |

Astro reads it from its project root, which is this directory under both
`pnpm --filter ui` and `astro --root ui`, and Vite does not search parent
directories: a `.env` at the workspace root is never read. The API resolves its
own file the same way, deliberately — see [`../api/README.md`](../api/README.md).

`ui/.env` does **not** reach a Bazel build. A file Bazel was not told about is
absent from the sandbox, so Astro never sees it; that is why the release config
has no default rather than a localhost one, which would otherwise ship a
deployed site pointing at the visitor's own machine.

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
