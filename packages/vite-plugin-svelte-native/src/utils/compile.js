import { log } from './log.js';
import { mapToRelative } from './sourcemaps.js';
import { enhanceCompileError } from './error.js';
import { filterWarnings, transformWithNative } from './native.js';

// TODO this is a patched version of https://github.com/sveltejs/vite-plugin-svelte/pull/796/files#diff-3bce0b33034aad4b35ca094893671f7e7ddf4d27254ae7b9b0f912027a001b15R10
// which is closer to the other regexes in at least not falling into commented script
// but ideally would be shared exactly with svelte and other tools that use it
const scriptLangRE =
	/<!--[^]*?-->|<script\s+(?:[^>]*|(?:[^=>'"/]+=(?:"[^"]*"|'[^']*'|[^>\s]+)\s+)*)lang=(["'])?([^"' >]+)\1[^>]*>/g;

/**
 * @returns {import('../types/compile.d.ts').CompileSvelte}
 */
export function createCompileSvelte() {
	/** @type {import('../types/vite-plugin-svelte-stats.d.ts').StatCollection | undefined} */
	let stats;

	return async function compileSvelte(svelteRequest, code, options, sourcemap) {
		const { filename, normalizedFilename, cssId, ssr, raw } = svelteRequest;
		const { emitCss = true } = options;

		if (options.stats) {
			if (options.isBuild) {
				if (!stats) {
					stats = options.stats.startCollection(`${ssr ? 'ssr' : 'dom'} compile`, {
						logInProgress: () => false
					});
				}
			} else {
				if (ssr && !stats) {
					stats = options.stats.startCollection('ssr compile');
				}
				if (!ssr && stats) {
					stats.finish();
					stats = undefined;
				}
			}
		}

		/** @type {import('svelte/compiler').CompileOptions & { warningFilter?: ((warning: any) => boolean) | undefined }} */
		const compileOptions = {
			...options.compilerOptions,
			filename,
			generate: ssr ? 'server' : 'client'
		};

		let finalCode = code;
		if (compileOptions.hmr && options.emitCss) {
			const closeStylePos = code.lastIndexOf('</style>');
			if (closeStylePos > -1) {
				finalCode = finalCode.slice(0, closeStylePos) + ' *{}' + finalCode.slice(closeStylePos);
			}
		}

		const dynamicCompileOptions = await options?.dynamicCompileOptions?.({
			filename,
			code: finalCode,
			compileOptions
		});
		if (dynamicCompileOptions && log.debug.enabled) {
			log.debug(
				`dynamic compile options for  ${filename}: ${JSON.stringify(dynamicCompileOptions)}`,
				undefined,
				'compile'
			);
		}
		const finalCompileOptions = dynamicCompileOptions
			? {
					...compileOptions,
					...dynamicCompileOptions
				}
			: compileOptions;
		if (sourcemap) {
			finalCompileOptions.sourcemap = sourcemap;
		}

		const endStat = stats?.start(filename);
		try {
			const nativeResult = await transformWithNative({
				id: filename,
				code: finalCode,
				ssr,
				hmr: Boolean(finalCompileOptions.hmr),
				target: 'vite',
				requestKind: 'svelte-component',
				compilerOptions: { ...finalCompileOptions, filename }
			});

			/** @type {import('svelte/compiler').CompileResult} */
			const compiled = {
				js: {
					code: nativeResult.code,
					map: nativeResult.map
				},
				css: nativeResult.css
					? {
							code: nativeResult.css.code,
							map: nativeResult.css.map,
							hasGlobal: nativeResult.css.hasGlobal ?? undefined
						}
					: undefined,
				warnings: filterWarnings(nativeResult.warnings, finalCompileOptions.warningFilter),
				metadata: {}
			};

			if (
				options.server?.config.experimental.hmrPartialAccept &&
				compiled.js.code.includes('import.meta.hot.accept(')
			) {
				compiled.js.code = compiled.js.code.replaceAll(
					'import.meta.hot.accept(',
					'import.meta.hot.acceptExports(["default"],'
				);
			}

			if (endStat) {
				endStat();
			}
			mapToRelative(compiled.js?.map, filename);
			mapToRelative(compiled.css?.map, filename);
			if (!raw) {
				const hasCss = compiled.css?.code?.trim()?.length ?? 0 > 0;
				if (emitCss && hasCss) {
					compiled.js.code += `\nimport ${JSON.stringify(cssId)};\n`;
				}
			}

			let lang = 'js';
			for (const match of code.matchAll(scriptLangRE)) {
				if (match[2]) {
					lang = match[2];
					break;
				}
			}

			return {
				filename,
				normalizedFilename,
				cssId,
				lang,
				compiled,
				ssr
			};
		} catch (e) {
			enhanceCompileError(e, code, options.preprocess);
			throw e;
		}
	};
}
