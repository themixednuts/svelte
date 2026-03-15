set shell := ["powershell.exe", "-NoLogo", "-Command"]

default:
  just --list

fetch-all:
  git -C svelte fetch --all --prune
  git -C kit fetch --all --prune
  git -C svelte-language-tools fetch --all --prune

pull-all:
  git -C svelte pull --ff-only origin main
  git -C kit pull --ff-only origin main
  git -C svelte-language-tools pull --ff-only origin master

js-deps:
  pnpm --dir svelte install --frozen-lockfile

js-output-snapshots:
  $env:SVELTE_REPO_ROOT=(Resolve-Path svelte).Path; cargo run -p svelte-compiler --example js_output_snapshots

test-js-output-snapshots:
  cargo test -p svelte-compiler --test js_output_snapshots -- --nocapture

test-snapshot-js-suite:
  $env:SVELTE_REPO_ROOT=(Resolve-Path svelte).Path; cargo test -p svelte-compiler --test compiler_fixtures snapshot_js_suite_ported -- --nocapture

refresh-js-parity: js-deps js-output-snapshots test-js-output-snapshots test-snapshot-js-suite

real-world-canaries:
  cargo run -p svelte-compiler --example real_world_canaries

real-world-canaries-smoke:
  $env:SVELTE_REAL_WORLD_FILTER='immich'; $env:SVELTE_REAL_WORLD_MAX_FILES='4'; cargo run -p svelte-compiler --example real_world_canaries
