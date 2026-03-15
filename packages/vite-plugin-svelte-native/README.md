# @mixednuts/vite-plugin-svelte-native

Native Vite plugin for the Rust Svelte compiler.

This package is ESM-only.

## Install

```bash
pnpm add -D svelte vite @mixednuts/vite-plugin-svelte-native
```

## Usage

```ts
import { defineConfig } from 'vite'
import { svelte } from '@mixednuts/vite-plugin-svelte-native'

export default defineConfig({
  plugins: [svelte()]
})
```

## Notes

- This package is the Vite-facing entrypoint.
- It loads the native bridge from `@mixednuts/svelte-vite-native`.
- Node is the primary runtime target.
- Bun may work where Node-API compatibility is available.
- Deno is not the target runtime for this package.
- The current implementation focuses on transform bridging. It does not yet mirror the full `@sveltejs/vite-plugin-svelte` feature set.
- The main remaining gaps are `svelte.config` parity, preprocess bridging, and publish-ready native binary distribution.
