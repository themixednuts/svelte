/**
 * @param {{
 *   moduleId?: string,
 *   load?: (moduleId: string) => Promise<any>
 * }} [options]
 */
export async function createNodeCompilerClient(options = {}) {
  const moduleId = options.moduleId ?? 'svelte-compiler-napi'
  const load = options.load ?? ((id) => import(id))
  const mod = await load(moduleId)
  const binding = mod?.default && typeof mod.default === 'object' ? { ...mod, ...mod.default } : mod

  for (const name of [
    'version',
    'compileJson',
    'compileJsonWithCallbacks',
    'compileModuleJson',
    'parseJson',
    'parseCssJson',
    'printJson',
    'printJsonWithCallbacks',
    'printSourceJson',
    'printSourceJsonWithCallbacks',
    'migrateJson'
  ]) {
    if (
      name === 'compileJsonWithCallbacks' ||
      name === 'printJsonWithCallbacks' ||
      name === 'printSourceJson' ||
      name === 'printSourceJsonWithCallbacks'
    ) {
      continue
    }
    if (typeof binding?.[name] !== 'function') {
      throw new TypeError(`Module '${moduleId}' does not export ${name}`)
    }
  }

  return {
    versionSync: binding.version,
    compileJsonSync: binding.compileJson,
    compileJsonWithCallbacksSync: binding.compileJsonWithCallbacks,
    compileModuleJsonSync: binding.compileModuleJson,
    parseJsonSync: binding.parseJson,
    parseCssJsonSync: binding.parseCssJson,
    printJsonSync: binding.printJson,
    printJsonWithCallbacksSync: binding.printJsonWithCallbacks,
    printSourceJsonSync: binding.printSourceJson,
    printSourceJsonWithCallbacksSync: binding.printSourceJsonWithCallbacks,
    migrateJsonSync: binding.migrateJson
  }
}
