# @svelte-rs/vite-rolldown-bridge

Runtime-agnostic JS helpers for driving the Rust Svelte bridge.

## Design

- `src/core.js`: runtime-neutral request classification + sync bridge client wrapper.
- `src/node.js`: Node-oriented convenience adapter for loading the bundler N-API module.
- `src/compiler-node.js`: Node-oriented convenience adapter for loading the compiler N-API module.
- `src/vite-plugin.js`: Vite plugin glue using an injected bridge client.
- `src/compat-svelte-compiler.js`: `svelte/compiler` compatibility surface for the sync compiler APIs.

Use `@svelte-rs/vite-rolldown-bridge` for runtime-neutral exports,
and `@svelte-rs/vite-rolldown-bridge/node` when you explicitly want the Node adapter.

## Vite usage (Node adapter)

```js
import { defineConfig } from 'vite'
import { createNodeBridgeClient } from '@svelte-rs/vite-rolldown-bridge/node'
import { svelteRustBridgePlugin } from '@svelte-rs/vite-rolldown-bridge/vite'

const client = await createNodeBridgeClient({
  moduleId: 'svelte-vite-rolldown-napi'
})

export default defineConfig({
  plugins: [svelteRustBridgePlugin({ client })]
})
```

## Existing JS plugin compatibility

The compatibility module can emulate the sync `svelte/compiler` surface
through the Rust bridge (`VERSION`, `compile`, `compileModule`, `parse`, `parseCss`, `print`, `migrate`, `walk`).

Current explicit gaps:
- `preprocess` is not bridged yet because the upstream JS API is callback-driven and async.
- JS callback options such as `cssHash` and `print` comment getters are not bridged yet.
