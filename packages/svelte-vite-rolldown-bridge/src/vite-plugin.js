import { classifyRequestId } from './core.js'

const SVELTE_REQUEST_FILTER = /\.svelte(?:\.js)?(?:\?.*)?$/
const VIRTUAL_STYLE_QUERY = '?svelte-rs&type=style'

/**
 * @param {{
 *   client: {
 *     classifyRequestId(id: string): string,
 *     transformSync(request: {id: string, code: string, ssr?: boolean, hmr?: boolean, target?: 'vite'|'rolldown'}): {code: string, mapJson?: string|null, css?: string|null}
 *   },
 *   emitCss?: boolean,
 *   hmr?: boolean
 * }} options
 */
export function svelteRustBridgePlugin(options) {
  const client = options?.client
  const emitCss = options?.emitCss ?? true
  const hmr = options?.hmr ?? true

  if (!client || typeof client.transformSync !== 'function' || typeof client.classifyRequestId !== 'function') {
    throw new TypeError('svelteRustBridgePlugin requires a bridge client with transformSync and classifyRequestId')
  }

  /** @type {Map<string, string>} */
  const cssVirtualModules = new Map()

  return {
    name: 'svelte-rs-bridge',
    enforce: 'pre',
    transform: {
      filter: {
        id: SVELTE_REQUEST_FILTER
      },
      handler(code, id) {
        const kind = client.classifyRequestId(id) || classifyRequestId(id)
        if (kind !== 'svelte-component' && kind !== 'svelte-module') {
          return
        }

        const requestId = id.split('?', 1)[0]
        const ssr = this.environment?.config?.consumer === 'server'
        const result = client.transformSync({
          id: requestId,
          code,
          ssr,
          hmr: hmr && !ssr,
          target: 'vite'
        })

        let transformedCode = result.code
        if (emitCss && kind === 'svelte-component' && result.css && !ssr) {
          const cssId = `${requestId}${VIRTUAL_STYLE_QUERY}`
          cssVirtualModules.set(cssId, result.css)
          transformedCode += `\nimport ${JSON.stringify(cssId)};\n`
        }

        return {
          code: transformedCode,
          map: parseMap(result.mapJson)
        }
      }
    },
    load(id) {
      if (cssVirtualModules.has(id)) {
        return cssVirtualModules.get(id)
      }
    }
  }
}

/**
 * @param {string | null | undefined} mapJson
 */
function parseMap(mapJson) {
  if (!mapJson) {
    return null
  }
  try {
    return JSON.parse(mapJson)
  } catch {
    return null
  }
}
