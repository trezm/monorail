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

// outDir and cacheDir are overridable because Bazel builds in a sandbox whose
// source tree is read-only: both have to land under bazel-out instead.
export default defineConfig({
  integrations: [react()],
  output: 'static',
  outDir: process.env.ASTRO_OUT_DIR ?? './dist',
  cacheDir: process.env.ASTRO_CACHE_DIR ?? './node_modules/.astro',
  server: { port: 4321 },
});
