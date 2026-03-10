/**
 * Runtime-agnostic request classification.
 * Keep behavior aligned with Rust bridge classification.
 * @param {string} id
 */
export function classifyRequestId(id) {
  const [path, query = ''] = id.split('?', 2)
  const pathLower = path.toLowerCase()
  const queryLower = query.toLowerCase()

  if ((pathLower.includes('.svelte') || queryLower.includes('svelte')) && queryLower.includes('type=style')) {
    return 'virtual-css'
  }
  if (pathLower.endsWith('.svelte.js')) {
    return 'svelte-module'
  }
  if (pathLower.endsWith('.svelte')) {
    return 'svelte-component'
  }
  return 'unknown'
}

/**
 * @param {string} id
 */
export function shouldTransformId(id) {
  const kind = classifyRequestId(id)
  return kind === 'svelte-component' || kind === 'svelte-module'
}

/**
 * @typedef {{
 *   id: string,
 *   code: string,
 *   ssr?: boolean,
 *   hmr?: boolean,
 *   target?: 'vite' | 'rolldown'
 * }} BridgeTransformRequest
 */

/**
 * @typedef {{
 *   code: string,
 *   mapJson?: string | null,
 *   css?: string | null
 * }} BridgeTransformResult
 */

/**
 * Create a synchronous bridge client from a transport function.
 * Transport is runtime-specific, client is runtime-agnostic.
 * @param {{
 *   transformJsonSync: (inputJson: string) => string,
 *   classifyRequestIdSync?: (id: string) => string
 * }} transport
 */
export function createBridgeClientSync(transport) {
  if (!transport || typeof transport.transformJsonSync !== 'function') {
    throw new TypeError('createBridgeClientSync requires transformJsonSync(inputJson)')
  }

  return {
    /** @param {string} id */
    classifyRequestId(id) {
      if (typeof transport.classifyRequestIdSync === 'function') {
        return normalizeKind(transport.classifyRequestIdSync(id))
      }
      return classifyRequestId(id)
    },

    /** @param {BridgeTransformRequest} request */
    transformSync(request) {
      const input = JSON.stringify({
        id: request.id,
        code: request.code,
        ssr: Boolean(request.ssr),
        hmr: Boolean(request.hmr),
        target: request.target ?? 'vite'
      })
      const output = transport.transformJsonSync(input)
      const parsed = JSON.parse(output)
      return {
        code: parsed.code,
        mapJson: parsed.map_json ?? null,
        css: parsed.css ?? null
      }
    }
  }
}

/**
 * @param {string} value
 */
function normalizeKind(value) {
  if (value === 'svelte-component' || value === 'svelte-module' || value === 'virtual-css' || value === 'unknown') {
    return value
  }
  return 'unknown'
}
