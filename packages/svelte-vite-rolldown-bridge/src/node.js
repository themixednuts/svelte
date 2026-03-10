import { createBridgeClientSync } from './core.js'

/**
 * Node-oriented convenience loader for N-API binding.
 * Runtime-agnostic core stays in `core.js`; this is only an adapter.
 *
 * @param {{
 *   moduleId?: string,
 *   load?: (moduleId: string) => Promise<any>
 * }} [options]
 */
export async function createNodeBridgeClient(options = {}) {
  const moduleId = options.moduleId ?? 'svelte-vite-rolldown-napi'
  const load = options.load ?? ((id) => import(id))
  const mod = await load(moduleId)
  const binding = mod?.default && typeof mod.default === 'object' ? { ...mod, ...mod.default } : mod

  if (typeof binding.transformJson !== 'function') {
    throw new TypeError(`Module '${moduleId}' does not export transformJson`)
  }

  return createBridgeClientSync({
    transformJsonSync: binding.transformJson,
    classifyRequestIdSync: binding.classifyRequestId
  })
}
