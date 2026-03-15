# @mixednuts/svelte-vite-native

Native Node-API wrapper for the Rust Svelte Vite bridge.

This package is ESM-only.

This package is the low-level runtime package. Most users should install
[`@mixednuts/vite-plugin-svelte-native`](../vite-plugin-svelte-native/README.md)
and use that plugin in `vite.config`.

## What it provides

- synchronous access to the native transform binding
- request classification helpers exposed by the Rust bridge
- lazy addon loading from ESM

## Runtime support

- Node: supported
- Bun: expected to work where Node-API compatibility is available
- Deno: not the target runtime for this package
