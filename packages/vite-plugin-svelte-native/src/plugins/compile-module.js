import { buildModuleIdFilter, buildModuleIdParser } from '../utils/id.js';
import { log, logCompilerWarnings } from '../utils/log.js';
import { toRollupError } from '../utils/error.js';
import { filterWarnings, transformWithNative } from '../utils/native.js';

/**
 * @param {import('../types/plugin-api.d.ts').PluginAPI} api
 * @returns {import('vite').Plugin}
 */
export function compileModule(api) {
	/**
	 * @type {import("../types/options.js").ResolvedOptions}
	 */
	let options;
	/**
	 * @type {import("../types/id.js").ModuleIdParser}
	 */
	let idParser;

	/**
	 * @type {import('svelte/compiler').ModuleCompileOptions}
	 */
	let staticModuleCompileOptions;

	/** @type {import('vite').Plugin} */
	const plugin = {
		name: 'vite-plugin-svelte:compile-module',
		enforce: 'post',
		async configResolved() {
			options = api.options;
			// @ts-expect-error transform defined below but filter not in type
			plugin.transform.filter = buildModuleIdFilter(options);
			idParser = buildModuleIdParser(options);
			staticModuleCompileOptions = filterNonModuleCompilerOptions(options.compilerOptions);
		},
		transform: {
			async handler(code, id) {
				const ssr = this.environment.config.consumer === 'server';
				const moduleRequest = idParser(id, ssr);
				if (!moduleRequest) {
					return;
				}
				const filename = moduleRequest.filename;
				/** @type {import('svelte/compiler').CompileOptions & { warningFilter?: ((warning: any) => boolean) | undefined }} */
				const compileOptions = {
					...staticModuleCompileOptions,
					dev: !this.environment.config.isProduction,
					generate: ssr ? 'server' : 'client',
					filename
				};
				const dynamicCompileOptions = await options?.dynamicCompileOptions?.({
					filename,
					code,
					compileOptions
				});
				if (dynamicCompileOptions && log.debug.enabled) {
					log.debug(
						`dynamic compile options for  ${filename}: ${JSON.stringify(dynamicCompileOptions)}`,
						undefined,
						'compileModule'
					);
				}
				const finalCompileOptions = dynamicCompileOptions
					? {
							...compileOptions,
							...dynamicCompileOptions
						}
					: compileOptions;
				if (dynamicCompileOptions?.experimental) {
					finalCompileOptions.experimental = {
						...compileOptions.experimental,
						...dynamicCompileOptions.experimental
					};
				}
				const finalModuleCompileOptions = filterNonModuleCompilerOptions(finalCompileOptions);
				if (log.debug.enabled) {
					log.debug(
						`final ModuleCompileOptions for  ${filename}: ${JSON.stringify(finalModuleCompileOptions)}`,
						undefined,
						'compileModule'
					);
				}
				try {
					const result = await transformWithNative({
						id: filename,
						code,
						ssr,
						hmr: false,
						target: 'vite',
						requestKind: 'svelte-module',
						compilerOptions: finalModuleCompileOptions
					});
					logCompilerWarnings(
						moduleRequest,
						filterWarnings(result.warnings, finalCompileOptions.warningFilter),
						options
					);
					return {
						code: result.code,
						map: result.map,
						moduleType: 'js'
					};
				} catch (e) {
					throw toRollupError(e, options);
				}
			}
		}
	};
	return plugin;
}

/**
 * @param {import('svelte/compiler').CompileOptions} compilerOptions
 * @return {import('svelte/compiler').ModuleCompileOptions}
 */
function filterNonModuleCompilerOptions(compilerOptions) {
	/** @type {Array<keyof import('svelte/compiler').ModuleCompileOptions>} */
	const knownModuleCompileOptionNames = [
		'dev',
		'generate',
		'filename',
		'rootDir',
		'sourcemap',
		'outputFilename',
		'warningFilterIgnoreCodes',
		'discloseVersion',
		'experimental'
	];
	const experimentalModuleCompilerOptionNames = ['async'];

	/** @type {import('svelte/compiler').ModuleCompileOptions} */
	const filtered = filterByPropNames(compilerOptions, knownModuleCompileOptionNames);
	if (filtered.experimental) {
		filtered.experimental = filterByPropNames(
			filtered.experimental,
			experimentalModuleCompilerOptionNames
		);
	}
	return filtered;
}

/**
 * @param {object} o
 * @param {string[]} names
 * @returns {object}
 */
function filterByPropNames(o, names) {
	return Object.fromEntries(Object.entries(o).filter(([name]) => names.includes(name)));
}
