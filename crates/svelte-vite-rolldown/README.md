# svelte-vite-rolldown

Native Rust bridge surface for eventual direct Vite/Rolldown integration.

Current state:

- Defines request/response types and bridge trait.
- Includes `RustCompilerBridge` dispatching to `svelte-compiler` for `.svelte` and `.svelte.js`.
- Includes request classification helpers for component/module/virtual-css ids.
- Includes JSON transport helper (`transform_json`) for host adapters.

Planned next steps:

1. Expose Node-facing adapter through the N-API host crate.
2. Add Vite plugin glue package using the bridge JSON protocol.
3. Validate behavior against Rolldown-compatible hook filters and module typing.
