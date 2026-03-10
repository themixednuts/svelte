'use strict'

const fs = require('node:fs')
const path = require('node:path')

const root = path.resolve(__dirname, '..', '..')
const depsDir = path.join(root, 'target', 'debug', 'deps')
const cacheDir = path.join(__dirname, '.native')
const addonPath = path.join(cacheDir, `svelte-compiler-napi.${process.platform}-${process.arch}.node`)

module.exports = loadNativeBinding()

function loadNativeBinding() {
  const sourcePath = findNativeSource()
  fs.mkdirSync(cacheDir, { recursive: true })

  if (!fs.existsSync(addonPath) || isSourceNewer(sourcePath, addonPath)) {
    fs.copyFileSync(sourcePath, addonPath)
  }

  return require(addonPath)
}

function findNativeSource() {
  const ext =
    process.platform === 'win32' ? '.dll' :
    process.platform === 'darwin' ? '.dylib' :
    '.so'

  const candidates = [
    path.join(depsDir, `svelte_compiler_napi${ext}`),
    path.join(root, 'target', 'release', 'deps', `svelte_compiler_napi${ext}`)
  ]

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate
    }
  }

  throw new Error(
    `Native compiler addon not found. Build crates/svelte-compiler-napi first. Checked: ${candidates.join(', ')}`
  )
}

function isSourceNewer(sourcePath, targetPath) {
  return fs.statSync(sourcePath).mtimeMs > fs.statSync(targetPath).mtimeMs
}
