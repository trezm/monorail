# ui

The web front end: [Astro](https://astro.build) with React islands.

```bash
bazel run //ui:dev     # http://localhost:4321
bazel build //ui       # static site into bazel-bin/ui/dist
```

One page: a Login with Railway button, or — once there is a session — the
account's Railway projects, each expanding to the services inside it.

## Layout

| Path | |
|---|---|
| `src/pages/index.astro` | The only route. Shell and nothing else. |
| `src/components/Dashboard.tsx` | Whether there is a login, and what to show either way. |
| `src/components/ProjectList.tsx` | The projects, and their services. |
| `src/components/LoginButton.tsx` | The button. |
| `src/lib/api.ts` | Every call to the API, and the shape of a failure. |
| `src/styles/global.css` | Everything visual. No framework. |
| `astro.config.mjs` | React integration, and the output paths Bazel overrides. |

The site is static, so who is signed in cannot be known at build time. One
island asks `GET /api/v1/users/me` on mount and renders the login or the
dashboard from the answer; a `checking` state in between is what stops a
signed-in visitor seeing a login button that is about to disappear.

A project is a native `<details>`, not a hand-rolled disclosure: it is keyboard
operable, announced as expandable, and findable by the browser's own in-page
search before any of this code runs. `open` is controlled from React so an
account with a single project can start expanded without the component and the
DOM disagreeing about the attribute afterwards.

The login button is an anchor, not a `<button>` with an `onClick`. An OAuth
redirect has to be a top-level navigation, so `fetch` cannot start one, and a
real link keeps middle-click, keyboard activation and the status bar working. It
is a React component anyway because the redirect takes a moment and the click
needs acknowledging.

`src/lib/api.ts` sends `credentials: 'include'` on every call — the session
cookie belongs to the API's origin, not this one — and turns a failure into an
`ApiError` carrying the API's own `code`. Branch on that, never on the message.
An `unauthorized` from `/api/v1/projects` means the Railway token behind the
session is spent, so the page falls back to the login rather than reporting an
error.

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
