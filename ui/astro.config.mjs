import react from '@astrojs/react';
import { defineConfig } from 'astro/config';

// outDir and cacheDir are overridable because Bazel builds in a sandbox whose
// source tree is read-only: both have to land under bazel-out instead.
export default defineConfig({
  integrations: [react()],
  output: 'static',
  outDir: process.env.ASTRO_OUT_DIR ?? './dist',
  cacheDir: process.env.ASTRO_CACHE_DIR ?? './node_modules/.astro',
  server: { port: 4321 },
});
