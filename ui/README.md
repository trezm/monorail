# ui

The web front end: [Astro](https://astro.build) with React islands.

```bash
bazel run //ui:dev     # http://localhost:4321
bazel build //ui       # static site into bazel-bin/ui/dist
```

One page: a Login with Railway button, or — once there is a session — an
account bar and the Railway projects on that account, each expanding to the
services inside it. An expanded project offers a dropdown of its environments
and shows how each service is configured and deployed in the selected one. A
service with a running deployment can be spun down from there, behind a
confirmation step, and a spun-down one brought back up.

## Layout

| Path | |
|---|---|
| `src/pages/index.astro` | The only route. |
| `src/components/App.tsx` | The one island, hydrated with `client:load`. |
| `src/components/LoginButton.tsx` | The button. |
| `src/components/UserMenu.tsx` | Name, avatar and log out, top right. |
| `src/components/ProjectList.tsx` | The projects, as expandable rows. |
| `src/components/ProjectServices.tsx` | One project's services, per selected environment. |
| `src/lib/session.tsx` | The session context, provider and hook. |
| `src/lib/projects.ts` | The `/api/v1` project reads, and the types they return. |
| `src/styles/global.css` | Everything visual. No framework. |
| `astro.config.mjs` | React integration, and the output paths Bazel overrides. |

The button is an anchor, not a `<button>` with an `onClick`. An OAuth redirect
has to be a top-level navigation, so `fetch` cannot start one, and a real link
keeps middle-click, keyboard activation and the status bar working. It is a
React component anyway because the redirect takes a moment and the click needs
acknowledging.

A project is a native `<details>`, not a hand-rolled disclosure: it is keyboard
operable, announced as expandable, and findable by the browser's own in-page
search before any of this code runs. `open` is controlled from React so an
account with a single project can start expanded without the component and the
DOM disagreeing about the attribute afterwards.

## The session

The site is static, so the build knows nothing about who is looking at it and
the browser has to ask. `SessionProvider` requests `GET /api/v1/users/me` once
with `credentials: 'include'`, which is what attaches the session cookie across
the two origins. A `401` is the ordinary answer for a visitor who has not logged
in, not an error.

That answer decides what renders. `session.isSignedIn()` and
`session.isSignedOut()` are not each other's negation — until the request
answers, both are false — so the account bar, the login button and the projects
each wait for their own answer instead of one of them showing to the wrong
person.

React context does not cross Astro island boundaries, so everything that reads
the session lives under a single `client:load` island rather than one per
component. That is what `App.tsx` is for, and it is why logging out needs no
page reload: `DELETE /auth/session` clears the `HttpOnly` cookie server-side —
nothing else can — and the provider then tells every component at once.

A `401` from any `/api/v1` project read means something else: the cookie is
still good and the Railway token behind it is spent. Nothing in the browser can
renew it, so the component that hit it ends the session through the provider,
which puts the login button back.

## Configuration

`PUBLIC_API_URL` is where the API lives. `PUBLIC_` is Astro's prefix for values
that reach the browser, which this one must: every request is made from the
page, so the URL is baked in at build time rather than read at runtime.

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

The API needs to allow this origin to send its session cookie, or every call
above the login fails:

```bash
API_CORS_ALLOWED_ORIGINS=http://localhost:4321
```

A wildcard will not do — the CORS specification forbids pairing credentials with
one. [`../api/.env.example`](../api/.env.example) already carries this value.

## Dependencies

`pnpm-lock.yaml` at the repo root is the source of truth, the way `Cargo.lock`
is for Rust, and `rules_js` mirrors it into Bazel. Add a dependency with:

```bash
pnpm --filter ui add <package>
```

Bazel picks up the rewritten lockfile on the next build; there is no repin step.

`pnpm install` at the root is only needed for the editor and for running
`astro` outside Bazel — the build materialises its own `node_modules`.
