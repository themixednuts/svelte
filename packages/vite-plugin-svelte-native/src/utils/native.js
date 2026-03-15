let nativeModulePromise;

/**
 * @returns {Promise<typeof import('@mixednuts/svelte-vite-native')>}
 */
async function loadNativeModule() {
	if (!nativeModulePromise) {
		nativeModulePromise = import('../../../svelte-vite-native/index.js').catch(async () => {
			return import('@mixednuts/svelte-vite-native');
		});
	}
	return nativeModulePromise;
}

/**
 * @typedef NativeCssResult
 * @property {string} code
 * @property {any} [map]
 * @property {boolean | null} [hasGlobal]
 *
 * @typedef NativeTransformResult
 * @property {string} code
 * @property {any} [map]
 * @property {NativeCssResult | null} [css]
 * @property {any[] | null} [warnings]
 */

/**
 * @param {object} request
 * @returns {Promise<NativeTransformResult>}
 */
export async function transformWithNative(request) {
	const native = await loadNativeModule();
	const result = await native.transformSync(request);
	return {
		code: result.code,
		map: parseMap(result.mapJson ?? result.map_json ?? null),
		css:
			result.css == null
				? null
				: {
						code: result.css,
						map: parseMap(result.cssMapJson ?? result.css_map_json ?? null),
						hasGlobal: result.cssHasGlobal ?? result.css_has_global ?? null
					},
		warnings: parseWarnings(result.warningsJson ?? result.warnings_json ?? null)
	};
}

/**
 * @param {string} code
 * @param {import('svelte/compiler').PreprocessorGroup | import('svelte/compiler').PreprocessorGroup[]} preprocessors
 * @param {{ filename?: string | undefined }} [options]
 */
export async function preprocessWithNative(code, preprocessors, options) {
	const native = await loadNativeModule();
	return native.preprocessAsync(code, preprocessors, options);
}

/**
 * @param {any[] | null | undefined} warnings
 * @param {((warning: any) => boolean) | undefined} warningFilter
 * @returns {any[]}
 */
export function filterWarnings(warnings, warningFilter) {
	if (!Array.isArray(warnings) || warnings.length === 0) {
		return [];
	}
	if (typeof warningFilter !== 'function') {
		return warnings;
	}
	return warnings.filter((warning) => warningFilter(warning));
}

/**
 * @param {string | null | undefined} mapJson
 * @returns {any}
 */
function parseMap(mapJson) {
	if (!mapJson) {
		return null;
	}
	return JSON.parse(mapJson);
}

/**
 * @param {string | null | undefined} warningsJson
 * @returns {any[] | null}
 */
function parseWarnings(warningsJson) {
	if (!warningsJson) {
		return null;
	}
	const parsed = JSON.parse(warningsJson);
	return Array.isArray(parsed) ? parsed : null;
}
