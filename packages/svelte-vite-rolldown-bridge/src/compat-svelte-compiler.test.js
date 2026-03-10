import test from 'node:test'
import assert from 'node:assert/strict'

import { createCompilerCompat } from './compat-svelte-compiler.js'

test('compile translates package options and normalizes output', () => {
  /** @type {string | undefined} */
  let seenOptions

  const compat = createCompilerCompat({
    versionSync: () => '5.53.5',
    compileJsonSync(source, optionsJson) {
      assert.equal(source, '<h1>Hello</h1>')
      seenOptions = optionsJson ?? undefined
      return JSON.stringify({
        js: { code: 'compiled', map: { version: 3, sources: [], names: [], mappings: '' } },
        css: { code: '.x{}', map: { version: 3, sources: [], names: [], mappings: '' }, has_global: false },
        warnings: [{ code: 'a', message: 'keep' }, { code: 'b', message: 'drop' }],
        metadata: { runes: false },
        ast: { type: 'Root', fragment: { type: 'Fragment', nodes: [] }, js: [], start: 0, end: 14, options: null, module: null, instance: null, css: null }
      })
    },
    compileModuleJsonSync() {
      throw new Error('unused')
    },
    parseJsonSync() {
      throw new Error('unused')
    },
    parseCssJsonSync() {
      throw new Error('unused')
    },
    printJsonSync() {
      throw new Error('unused')
    },
    migrateJsonSync() {
      throw new Error('unused')
    }
  })

  const result = compat.compile('<h1>Hello</h1>', {
    filename: 'Component.svelte',
    rootDir: 'E:/Projects/svelte',
    generate: false,
    modernAst: true,
    warningFilter: (warning) => warning.code === 'a'
  })

  assert.deepEqual(JSON.parse(seenOptions), {
    filename: 'Component.svelte',
    root_dir: 'E:/Projects/svelte',
    generate: 'none',
    modern_ast: true,
    sourcemap: { version: 3, sources: [], names: [], mappings: '' }
  })
  assert.equal(result.css.hasGlobal, false)
  assert.deepEqual(result.warnings.map((warning) => warning.code), ['a'])
})

test('parse modern ast can be printed later', () => {
  /** @type {Array<any>} */
  const printCalls = []

  const compat = createCompilerCompat({
    versionSync: () => '5.53.5',
    compileJsonSync() {
      throw new Error('unused')
    },
    compileModuleJsonSync() {
      throw new Error('unused')
    },
    parseJsonSync() {
      return JSON.stringify({
        type: 'Root',
        fragment: {
          type: 'Fragment',
          nodes: [{ type: 'Text', start: 0, end: 5, raw: 'Hello', data: 'Hello' }]
        },
        js: [],
        start: 0,
        end: 5,
        options: null,
        module: null,
        instance: null,
        css: null
      })
    },
    parseCssJsonSync() {
      throw new Error('unused')
    },
    printJsonSync(kind, source, astJson, optionsJson) {
      printCalls.push({ kind, source, astJson: JSON.parse(astJson), options: JSON.parse(optionsJson ?? '{}') })
      return JSON.stringify({ code: 'Hello', map: { version: 3, sources: [], names: [], mappings: '' } })
    },
    migrateJsonSync() {
      throw new Error('unused')
    }
  })

  const ast = compat.parse('Hello', { modern: true })
  const text = ast.fragment.nodes[0]
  const printed = compat.print(text, { preserveWhitespace: true })

  assert.equal(printed.code, 'Hello')
  assert.deepEqual(printCalls, [
    {
      kind: 'node',
      source: 'Hello',
      astJson: { type: 'Text', start: 0, end: 5, raw: 'Hello', data: 'Hello' },
      options: { preserve_whitespace: true }
    }
  ])
})

test('bridge errors are normalized into compiler-shaped errors', () => {
  const compat = createCompilerCompat({
    versionSync: () => '5.53.5',
    compileJsonSync() {
      const error = new Error(JSON.stringify({ code: 'parse-error', message: 'bad input', position: { start: 1, end: 2 } }))
      throw error
    },
    compileModuleJsonSync() {
      throw new Error('unused')
    },
    parseJsonSync() {
      throw new Error('unused')
    },
    parseCssJsonSync() {
      throw new Error('unused')
    },
    printJsonSync() {
      throw new Error('unused')
    },
    migrateJsonSync() {
      throw new Error('unused')
    }
  })

  assert.throws(
    () => compat.compile('<h1>Hello</h1>'),
    (error) => error instanceof Error && error.message === 'bad input' && error.code === 'parse-error'
  )
})

test('cssHash callback is bridged through compile callback transport', () => {
  const expectedHash = hashLikeRust('Component.svelte')
  const compat = createCompilerCompat({
    versionSync: () => '5.53.5',
    compileJsonSync() {
      throw new Error('wrong path')
    },
    compileJsonWithCallbacksSync(_source, _optionsJson, cssHashCallback) {
      const cssHash = cssHashCallback(
        JSON.stringify({
          name: 'Component',
          filename: 'Component.svelte',
          css: '.x{}',
          hash_input: 'Component.svelte'
        })
      )
      assert.equal(cssHash, `scoped-${expectedHash}`)
      return JSON.stringify({
        js: { code: 'compiled', map: { version: 3, sources: [], names: [], mappings: '' } },
        css: null,
        warnings: [],
        metadata: { runes: false },
        ast: null
      })
    },
    compileModuleJsonSync() {
      throw new Error('unused')
    },
    parseJsonSync() {
      throw new Error('unused')
    },
    parseCssJsonSync() {
      throw new Error('unused')
    },
    printJsonSync() {
      throw new Error('unused')
    },
    migrateJsonSync() {
      throw new Error('unused')
    }
  })

  compat.compile('<style>.x{}</style>', {
    cssHash({ hash, filename }) {
      return `scoped-${hash(filename)}`
    }
  })
})

/**
 * @param {string} input
 */
function hashLikeRust(input) {
  const normalized = input.replace(/\r/g, '')
  let hash = 5381 >>> 0
  for (let i = normalized.length - 1; i >= 0; i -= 1) {
    hash = (((hash << 5) - hash) ^ normalized.charCodeAt(i)) >>> 0
  }
  return hash.toString(36)
}

test('print comment callbacks are bridged through print callback transport', () => {
  const compat = createCompilerCompat({
    versionSync: () => '5.53.5',
    compileJsonSync() {
      throw new Error('unused')
    },
    compileJsonWithCallbacksSync() {
      throw new Error('unused')
    },
    compileModuleJsonSync() {
      throw new Error('unused')
    },
    parseJsonSync() {
      return JSON.stringify({
        type: 'Root',
        fragment: { type: 'Fragment', nodes: [] },
        js: [
          {
            type: 'Script',
            start: 0,
            end: 10,
            context: 'default',
            attributes: [],
            content: {
              type: 'Program',
              body: [
                { type: 'ExpressionStatement', start: 8, end: 9, expression: { type: 'Literal', start: 8, end: 9, value: 1, raw: '1' } }
              ],
              sourceType: 'module'
            }
          }
        ],
        start: 0,
        end: 18,
        options: null,
        module: null,
        instance: {
          type: 'Script',
          start: 0,
          end: 18,
          context: 'default',
          attributes: [],
          content: {
            type: 'Program',
            body: [
              { type: 'ExpressionStatement', start: 8, end: 9, expression: { type: 'Literal', start: 8, end: 9, value: 1, raw: '1' } }
            ],
            sourceType: 'module'
          }
        },
        css: null,
        comments: []
      })
    },
    parseCssJsonSync() {
      throw new Error('unused')
    },
    printJsonSync() {
      throw new Error('unused')
    },
    printJsonWithCallbacksSync(_kind, _source, _astJson, _optionsJson, leading) {
      const comments = JSON.parse(leading(JSON.stringify({ type: 'ExpressionStatement' })))
      assert.deepEqual(comments, [{ type: 'Line', value: ' comment', start: 0, end: 0, loc: null }])
      return JSON.stringify({ code: '<script>\n\t// comment\n\t1;\n</script>', map: { version: 3, sources: [], names: [], mappings: '' } })
    },
    migrateJsonSync() {
      throw new Error('unused')
    }
  })

  const ast = compat.parse('<script>1</script>', { modern: true })
  const result = compat.print(ast.instance, {
    getLeadingComments() {
      return [{ type: 'Line', value: ' comment', start: 0, end: 0, loc: null }]
    }
  })

  assert.match(result.code, /comment/)
})
