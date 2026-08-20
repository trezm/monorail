import { dirname } from 'node:path';

import react from '@astrojs/react';
import { defineConfig } from 'astro/config';

// Every Bazel target sets PUBLIC_API_URL, so an empty value means a build that
// was never told where the API lives — `--config=release` without a
// `--define`. Undefined is a plain `pnpm` run instead, where Astro loads
// `ui/.env` itself and the component's development default applies.
if (process.env.PUBLIC_API_URL === '') {
  throw new Error(
    'PUBLIC_API_URL is empty. Pass --define=PUBLIC_API_URL=https://api.example.com ' +
      'to the build; a release must not fall back to localhost.',
  );
}

// Vite serves only what is under its allow list, and `bazel run //ui:dev`
// straddles two trees: js_run_devserver copies the sources into a temp
// directory, while node_modules there are symlinks back out to the execroot.
// Allowing one root 403s the other — the sources, or every client-side
// renderer including @astrojs/react. A refused renderer fails to hydrate, which
// is invisible until a component renders nothing without JavaScript.
//
// rules_js sets JS_BINARY__EXECROOT only under Bazel, so a plain `pnpm dev`
// keeps Vite's own default.
const execroot = process.env.JS_BINARY__EXECROOT;
const serverRoots = execroot ? [dirname(process.cwd()), execroot] : [];

// outDir and cacheDir are overridable because Bazel builds in a sandbox whose
// source tree is read-only: both have to land under bazel-out instead.
export default defineConfig({
  integrations: [react()],
  output: 'static',
  outDir: process.env.ASTRO_OUT_DIR ?? './dist',
  cacheDir: process.env.ASTRO_CACHE_DIR ?? './node_modules/.astro',
  server: { port: 4321 },
  vite: serverRoots.length ? { server: { fs: { allow: serverRoots } } } : {},
});
