import { createNodeCompilerClient } from './compiler-node.js'

const PRINT_SOURCE = new WeakMap()
const PRINT_SOURCE_BY_AST_JSON = new Map()

/** @type {string} */
export const VERSION = '5.53.5'

/**
 * @param {{
 *   versionSync: () => string,
 *   compileJsonSync: (source: string, optionsJson?: string | null) => string,
 *   compileJsonWithCallbacksSync?: (source: string, optionsJson?: string | null, cssHashCallback?: ((payloadJson: string) => string) | null) => string,
 *   compileModuleJsonSync: (source: string, optionsJson?: string | null) => string,
 *   parseJsonSync: (source: string, optionsJson?: string | null) => string,
 *   parseCssJsonSync: (source: string) => string,
 *   printJsonSync: (kind: string, source: string, astJson: string, optionsJson?: string | null) => string,
 *   printJsonWithCallbacksSync?: (kind: string, source: string, astJson: string, optionsJson?: string | null, leading?: ((nodeJson: string) => string) | null, trailing?: ((nodeJson: string) => string) | null) => string,
 *   printSourceJsonSync?: (source: string, optionsJson?: string | null) => string,
 *   printSourceJsonWithCallbacksSync?: (source: string, optionsJson?: string | null, leading?: ((nodeJson: string) => string) | null, trailing?: ((nodeJson: string) => string) | null) => string,
 *   migrateJsonSync: (source: string, optionsJson?: string | null) => string
 * }} client
 */
export function createCompilerCompat(client) {
  if (!client || typeof client.compileJsonSync !== 'function') {
    throw new TypeError('createCompilerCompat requires a compiler bridge client')
  }

  const resolvedVersion = typeof client.versionSync === 'function' ? client.versionSync() : VERSION

  return {
    VERSION: resolvedVersion,

    compile(source, options = {}) {
      source = removeBom(source)
      const { warningFilter, cssHash, ...rest } = options
      const translated = translateCompileOptions(rest)
      const result = invokeBridgeJson(() => {
        if (typeof cssHash === 'function' && typeof client.compileJsonWithCallbacksSync === 'function') {
          return client.compileJsonWithCallbacksSync(source, JSON.stringify(translated), (payloadJson) => {
            const payload = parseJsonResult(payloadJson)
            return cssHash({
              ...payload,
              hash: svelteHash
            })
          })
        }

        return client.compileJsonSync(source, JSON.stringify(translated))
      })
      normalizeCompileResult(result, source, Boolean(rest.modernAst))
      if (typeof warningFilter === 'function') {
        result.warnings = result.warnings.filter((warning) => warningFilter(warning))
      }
      return result
    },

    compileModule(source, options = {}) {
      source = removeBom(source)
      const { warningFilter, ...rest } = options

      const result = invokeBridgeJson(() =>
        client.compileModuleJsonSync(source, JSON.stringify(translateCompileOptions(rest)))
      )
      normalizeCompileResult(result, source, false)
      if (typeof warningFilter === 'function') {
        result.warnings = result.warnings.filter((warning) => warningFilter(warning))
      }
      return result
    },

    parse(source, options = {}) {
      source = removeBom(source)
      const ast = invokeBridgeJson(() =>
        client.parseJsonSync(source, JSON.stringify(translateParseOptions(options)))
      )
      if (options.modern) {
        registerPrintContext(ast, source)
      }
      return ast
    },

    parseCss(source) {
      return invokeBridgeJson(() => client.parseCssJsonSync(removeBom(source)))
    },

    print(ast, options = {}) {
      const source = lookupPrintSource(ast)
      if (!source) {
        throw new TypeError(
          'print(ast) requires a modern AST node produced by the Rust bridge parse/compile APIs'
        )
      }

      if (
        ast?.type === 'Root' &&
        Array.isArray(ast.comments) &&
        ast.comments.length > 0 &&
        typeof options.getLeadingComments !== 'function' &&
        typeof options.getTrailingComments !== 'function'
      ) {
        return {
          code: source,
          map: emptySourceMap()
        }
      }

      const result = invokeBridgeJson(() => {
        if (ast?.type === 'Root') {
          if (
            typeof client.printSourceJsonWithCallbacksSync === 'function' &&
            (typeof options.getLeadingComments === 'function' || typeof options.getTrailingComments === 'function')
          ) {
            return client.printSourceJsonWithCallbacksSync(
              source,
              JSON.stringify(translatePrintOptions(options)),
              typeof options.getLeadingComments === 'function'
                ? (nodeJson) => JSON.stringify(options.getLeadingComments(parseJsonResult(nodeJson)))
                : null,
              typeof options.getTrailingComments === 'function'
                ? (nodeJson) => JSON.stringify(options.getTrailingComments(parseJsonResult(nodeJson)))
                : null
            )
          }

          if (typeof client.printSourceJsonSync === 'function') {
            return client.printSourceJsonSync(source, JSON.stringify(translatePrintOptions(options)))
          }
        }

        if (
          typeof client.printJsonWithCallbacksSync === 'function' &&
          (typeof options.getLeadingComments === 'function' || typeof options.getTrailingComments === 'function')
        ) {
          return client.printJsonWithCallbacksSync(
            classifyPrintKind(ast),
            source,
            JSON.stringify(ast),
            JSON.stringify(translatePrintOptions(options)),
            typeof options.getLeadingComments === 'function'
              ? (nodeJson) => JSON.stringify(options.getLeadingComments(parseJsonResult(nodeJson)))
              : null,
            typeof options.getTrailingComments === 'function'
              ? (nodeJson) => JSON.stringify(options.getTrailingComments(parseJsonResult(nodeJson)))
              : null
          )
        }

        return client.printJsonSync(
          classifyPrintKind(ast),
          source,
          JSON.stringify(ast),
          JSON.stringify(translatePrintOptions(options))
        )
      })
      return result
    },

    migrate(source, options = {}) {
      source = removeBom(source)
      return invokeBridgeJson(() =>
        client.migrateJsonSync(source, JSON.stringify(translateMigrateOptions(options)))
      )
    },

    walk() {
      throw new Error(
        "'svelte/compiler' no longer exports a `walk` utility — please import it directly from 'estree-walker' instead"
      )
    },

    preprocess() {
      throw new Error('preprocess must be provided by the package wrapper')
    }
  }
}

/**
 * @param {{
 *   moduleId?: string,
 *   load?: (moduleId: string) => Promise<any>
 * }} [options]
 */
export async function createNodeCompilerCompat(options = {}) {
  return createCompilerCompat(await createNodeCompilerClient(options))
}

/**
 * @param {Record<string, any>} result
 * @param {string} source
 * @param {boolean} modernAst
 */
function normalizeCompileResult(result, source, modernAst) {
  result.js = normalizeArtifact(result.js)
  result.css = result.css ? normalizeArtifact(result.css) : null
  result.warnings = Array.isArray(result.warnings) ? result.warnings : []
  if (modernAst && result.ast && typeof result.ast === 'object') {
    registerPrintContext(result.ast, source)
  }
}

/**
 * @param {Record<string, any>} artifact
 */
function normalizeArtifact(artifact) {
  if (!artifact || typeof artifact !== 'object') {
    return artifact
  }
  if ('has_global' in artifact) {
    artifact.hasGlobal = artifact.has_global
    delete artifact.has_global
  }
  return artifact
}

/**
 * @param {string} source
 * @param {any} node
 */
function annotatePrintContext(node, source) {
  const visit = (value) => {
    if (!value || typeof value !== 'object') return
    if (PRINT_SOURCE.has(value)) return
    PRINT_SOURCE.set(value, source)
    if (Array.isArray(value)) {
      for (const item of value) visit(item)
      return
    }
    for (const child of Object.values(value)) {
      visit(child)
    }
  }

  visit(node)
}

/**
 * @param {any} ast
 * @param {string} source
 */
function registerPrintContext(ast, source) {
  annotatePrintContext(ast, source)
  const key = trySerializeAstKey(ast)
  if (key !== null) {
    PRINT_SOURCE_BY_AST_JSON.set(key, source)
  }
}

/**
 * @param {any} ast
 * @returns {string | undefined}
 */
function lookupPrintSource(ast) {
  const direct = PRINT_SOURCE.get(ast)
  if (direct) return direct

  const key = trySerializeAstKey(ast)
  if (key !== null) {
    return PRINT_SOURCE_BY_AST_JSON.get(key)
  }
}

/**
 * @param {any} ast
 * @returns {string | null}
 */
function trySerializeAstKey(ast) {
  try {
    return JSON.stringify(ast)
  } catch {
    return null
  }
}

/**
 * @param {any} ast
 * @returns {'root'|'fragment'|'node'|'script'|'css'|'css-node'|'attribute'|'options'}
 */
function classifyPrintKind(ast) {
  const type = ast?.type

  if (type === 'Root') return 'root'
  if (type === 'Fragment') return 'fragment'
  if (type === 'Script') return 'script'
  if (type === 'StyleSheet') return 'css'
  if (type === 'Rule' || type === 'Atrule') return 'css-node'
  if (
    type === 'Attribute' ||
    type === 'SpreadAttribute' ||
    type === 'BindDirective' ||
    type === 'OnDirective' ||
    type === 'ClassDirective' ||
    type === 'LetDirective' ||
    type === 'StyleDirective' ||
    type === 'TransitionDirective' ||
    type === 'AnimateDirective' ||
    type === 'UseDirective' ||
    type === 'AttachTag'
  ) {
    return 'attribute'
  }
  if (type === 'Comment') return 'comment'
  if (
    type === 'Text' ||
    type === 'IfBlock' ||
    type === 'EachBlock' ||
    type === 'KeyBlock' ||
    type === 'AwaitBlock' ||
    type === 'SnippetBlock' ||
    type === 'RenderTag' ||
    type === 'HtmlTag' ||
    type === 'ConstTag' ||
    type === 'DebugTag' ||
    type === 'ExpressionTag' ||
    type === 'RegularElement' ||
    type === 'Component' ||
    type === 'SlotElement' ||
    type === 'SvelteHead' ||
    type === 'SvelteBody' ||
    type === 'SvelteWindow' ||
    type === 'SvelteDocument' ||
    type === 'SvelteComponent' ||
    type === 'SvelteElement' ||
    type === 'SvelteSelf' ||
    type === 'SvelteFragment' ||
    type === 'SvelteBoundary' ||
    type === 'TitleElement'
  ) {
    return 'node'
  }
  if (Array.isArray(ast?.attributes) && !('name' in ast) && ('customElement' in ast || 'runes' in ast)) {
    return 'options'
  }

  throw new TypeError(`Unsupported modern AST node for print(): ${String(type ?? '<unknown>')}`)
}

/**
 * @param {any} options
 */
function translateCompileOptions(options) {
  const translated = {}

  copyIfPresent(translated, 'name', options.name)
  copyIfPresent(translated, 'filename', options.filename)
  copyIfPresent(translated, 'root_dir', options.rootDir)
  copyIfPresent(translated, 'fragments', options.fragments)
  copyIfPresent(translated, 'dev', options.dev)
  copyIfPresent(translated, 'hmr', options.hmr)
  copyIfPresent(translated, 'custom_element', options.customElement)
  copyIfPresent(translated, 'accessors', options.accessors)
  copyIfPresent(translated, 'namespace', options.namespace)
  copyIfPresent(translated, 'immutable', options.immutable)
  copyIfPresent(translated, 'css', options.css)
  copyIfPresent(translated, 'css_hash', options.cssHash)
  copyIfPresent(translated, 'runes', options.runes)
  copyIfPresent(translated, 'preserve_comments', options.preserveComments)
  copyIfPresent(translated, 'preserve_whitespace', options.preserveWhitespace)
  copyIfPresent(translated, 'disclose_version', options.discloseVersion)
  copyIfPresent(translated, 'modern_ast', options.modernAst)

  if ('generate' in options) {
    translated.generate = options.generate === false ? 'none' : options.generate
  }

  if (Array.isArray(options.warningFilterIgnoreCodes)) {
    translated.warning_filter_ignore_codes = options.warningFilterIgnoreCodes
  }

  if (options.compatibility && typeof options.compatibility === 'object') {
    translated.compatibility = {}
    copyIfPresent(translated.compatibility, 'component_api', options.compatibility.componentApi)
  }

  if (options.experimental && typeof options.experimental === 'object') {
    translated.experimental = {}
    copyIfPresent(translated.experimental, 'async', options.experimental.async)
  }

  translated.sourcemap = {
    version: 3,
    sources: [],
    names: [],
    mappings: ''
  }

  return translated
}

/**
 * @param {any} options
 */
function translateParseOptions(options) {
  const translated = {}
  copyIfPresent(translated, 'filename', options.filename)
  copyIfPresent(translated, 'root_dir', options.rootDir)
  copyIfPresent(translated, 'modern', options.modern)
  copyIfPresent(translated, 'loose', options.loose)
  return translated
}

/**
 * @param {any} options
 */
function translatePrintOptions(options) {
  const translated = {}
  copyIfPresent(translated, 'preserve_whitespace', options.preserveWhitespace)
  return translated
}

/**
 * @param {any} options
 */
function translateMigrateOptions(options) {
  const translated = {}
  copyIfPresent(translated, 'filename', options.filename)
  copyIfPresent(translated, 'use_ts', options.use_ts)
  return translated
}

/**
 * @param {Record<string, any>} target
 * @param {string} key
 * @param {any} value
 */
function copyIfPresent(target, key, value) {
  if (value !== undefined) {
    target[key] = value
  }
}

/**
 * @param {string} action
 */
function unsupportedCallback(action) {
  return new Error(`${action} is not available through the Rust JS compatibility layer yet because it requires JS callback bridging`)
}

/**
 * @param {string} json
 */
function parseJsonResult(json) {
  try {
    return JSON.parse(json)
  } catch (error) {
    throw new Error(`Invalid compiler bridge JSON response: ${error instanceof Error ? error.message : String(error)}`)
  }
}

/**
 * @template T
 * @param {() => string} fn
 * @returns {T}
 */
function invokeBridgeJson(fn) {
  try {
    return parseJsonResult(fn())
  } catch (error) {
    throw normalizeBridgeError(error)
  }
}

/**
 * @param {unknown} error
 */
function normalizeBridgeError(error) {
  const reason =
    error && typeof error === 'object' && 'reason' in error
      ? error.reason
      : error instanceof Error
        ? error.message
        : String(error)

  if (typeof reason === 'string') {
    try {
      const parsed = JSON.parse(reason)
      if (parsed && typeof parsed === 'object' && typeof parsed.message === 'string') {
        const enriched = new Error(parsed.message)
        for (const [key, value] of Object.entries(parsed)) {
          if (key !== 'message') {
            enriched[key] = value
          }
        }
        return enriched
      }
    } catch {
      // fall through
    }
  }

  return error instanceof Error ? error : new Error(String(error))
}

/**
 * Matches the Rust compiler hash implementation.
 * @param {string} input
 */
function svelteHash(input) {
  const normalized = input.replace(/\r/g, '')
  let hash = 5381 >>> 0
  for (let i = normalized.length - 1; i >= 0; i -= 1) {
    hash = (((hash << 5) - hash) ^ normalized.charCodeAt(i)) >>> 0
  }
  return hash.toString(36)
}

/**
 * @param {string} source
 */
function removeBom(source) {
  return source.charCodeAt(0) === 0xfeff ? source.slice(1) : source
}

function emptySourceMap() {
  return {
    version: 3,
    sources: [],
    names: [],
    mappings: ''
  }
}
