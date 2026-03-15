import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import preprocess from './src/vendor/preprocess/index.js'

const require = createRequire(import.meta.url)
const dirname = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(dirname, '..', '..')
const depsDir = path.join(root, 'target', 'debug', 'deps')
const cacheDir = path.join(dirname, '.native')
const addonPath = path.join(
  cacheDir,
  `svelte-vite-native.${process.platform}-${process.arch}.${process.pid}.node`
)
let cachedBinding
const STRUCTURED_ERROR_PREFIX = '__SVELTE_NATIVE_ERROR__'

export function loadNativeBinding() {
  if (cachedBinding) {
    return cachedBinding
  }
  const sourcePath = findNativeSource()
  fs.mkdirSync(cacheDir, { recursive: true })

  if (!fs.existsSync(addonPath) || nativeBinaryChanged(sourcePath, addonPath)) {
    fs.copyFileSync(sourcePath, addonPath)
  }

  cachedBinding = require(addonPath)
  return cachedBinding
}

export function transformSync(request) {
  const binding = loadNativeBinding()
  const prepared = prepareTransformRequest(request)
  try {
    if (prepared.cssHashCallback || prepared.warningFilterCallback) {
      return normalizeTransformResult(
        binding.transformSyncWithCallbacks(
          prepared.request,
          prepared.cssHashCallback,
          prepared.warningFilterCallback
        )
      )
    }
    return normalizeTransformResult(binding.transformSync(prepared.request))
  } catch (error) {
    throw normalizeNativeError(error)
  }
}

export function transformJson(inputJson) {
  try {
    return loadNativeBinding().transformJson(inputJson)
  } catch (error) {
    throw normalizeNativeError(error)
  }
}

export async function preprocessAsync(code, preprocessors, options = {}) {
  return preprocess(code, preprocessors, options)
}

export function classifyRequestId(id) {
  return loadNativeBinding().classifyRequestId(id)
}

export default {
  loadNativeBinding,
  transformSync,
  transformJson,
  preprocessAsync,
  classifyRequestId
}

function findNativeSource() {
  const explicitPath = process.env.SVELTE_NATIVE_ADDON_PATH
  if (explicitPath) {
    if (!fs.existsSync(explicitPath)) {
      throw new Error(`Native Vite bridge addon not found at SVELTE_NATIVE_ADDON_PATH=${explicitPath}`)
    }
    return explicitPath
  }

  const ext =
    process.platform === 'win32' ? '.dll' :
    process.platform === 'darwin' ? '.dylib' :
    '.so'

  const targetDir = process.env.CARGO_TARGET_DIR
  const debugDepsDir = targetDir
    ? path.join(targetDir, 'debug', 'deps')
    : depsDir
  const releaseDepsDir = targetDir
    ? path.join(targetDir, 'release', 'deps')
    : path.join(root, 'target', 'release', 'deps')

  const candidates = [
    path.join(debugDepsDir, `svelte_vite_rolldown_napi${ext}`),
    path.join(releaseDepsDir, `svelte_vite_rolldown_napi${ext}`)
  ]

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate
    }
  }

  throw new Error(
    `Native Vite bridge addon not found. Build crates/vite-rolldown-napi first. Checked: ${candidates.join(', ')}`
  )
}

function nativeBinaryChanged(sourcePath, targetPath) {
  const source = fs.statSync(sourcePath)
  const target = fs.statSync(targetPath)
  return source.mtimeMs !== target.mtimeMs || source.size !== target.size
}

function normalizeTransformResult(result) {
  return {
    code: result.code,
    mapJson: result.mapJson ?? result.map_json ?? null,
    css: result.css ?? null,
    cssMapJson: result.cssMapJson ?? result.css_map_json ?? null,
    cssHasGlobal: result.cssHasGlobal ?? result.css_has_global ?? null,
    warningsJson: result.warningsJson ?? result.warnings_json ?? null
  }
}

function normalizeNativeError(error) {
  const message = error instanceof Error ? error.message : String(error)
  if (!message.startsWith(STRUCTURED_ERROR_PREFIX)) {
    return error instanceof Error ? error : new Error(message)
  }

  const payload = JSON.parse(message.slice(STRUCTURED_ERROR_PREFIX.length))
  const normalized = new Error(payload.message ?? 'Svelte native compiler error')
  normalized.name = 'SvelteNativeError'
  Object.assign(normalized, payload)
  return normalized
}

function prepareTransformRequest(request) {
  const compilerOptions = request.compilerOptions ?? parseCompilerOptionsJson(request.compilerOptionsJson)
  const { sanitized, cssHashCallback, warningFilterCallback } = stripCompilerOptionCallbacks(compilerOptions)

  return {
    request: {
      id: request.id,
      code: request.code,
      ssr: request.ssr,
      hmr: request.hmr,
      target: normalizeBundlerTarget(request.target),
      requestKind: request.requestKind,
      compilerOptionsJson: sanitized ? JSON.stringify(sanitized) : request.compilerOptionsJson ?? null
    },
    cssHashCallback,
    warningFilterCallback
  }
}

function normalizeBundlerTarget(target) {
  if (target === 'rolldown' || target === 'Rolldown') {
    return 'Rolldown'
  }
  return 'Vite'
}

function parseCompilerOptionsJson(json) {
  if (!json) {
    return null
  }
  return JSON.parse(json)
}

function stripCompilerOptionCallbacks(compilerOptions) {
  if (!compilerOptions || typeof compilerOptions !== 'object') {
    return {
      sanitized: compilerOptions,
      cssHashCallback: undefined,
      warningFilterCallback: undefined
    }
  }

  const sanitized = { ...compilerOptions }
  const cssHashCallback =
    typeof sanitized.cssHash === 'function' ? sanitized.cssHash : undefined
  const warningFilterCallback =
    typeof sanitized.warningFilter === 'function' ? sanitized.warningFilter : undefined

  delete sanitized.cssHash
  delete sanitized.warningFilter

  return {
    sanitized,
    cssHashCallback,
    warningFilterCallback
  }
}
