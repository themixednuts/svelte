# svelte-vite-rolldown-napi

N-API host crate for the Rust Vite/Rolldown bridge.

Exports Node-callable functions that wrap `svelte-vite-rolldown`:

- `transform_sync(request)`
- `transform_json(input_json)`
- `classify_request_id(id)`

This crate keeps transport and runtime concerns separate from compiler logic.
