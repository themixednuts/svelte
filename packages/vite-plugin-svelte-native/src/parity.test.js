import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build, createServer, mergeConfig } from 'vite';

import { svelte as nativeSvelte, vitePreprocess as nativeVitePreprocess } from './index.js';

const thisDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(thisDir, '..', '..', '..');
const upstreamPluginUrl = pathToFileURL(
	path.join(
		repoRoot,
		'vite-plugin-svelte',
		'packages',
		'vite-plugin-svelte',
		'src',
		'index.js'
	)
).href;

async function loadUpstreamSvelte() {
	const module = await import(upstreamPluginUrl);
	return module.svelte;
}

async function loadUpstreamModule() {
	return import(upstreamPluginUrl);
}

async function createParityTempDir(name) {
	const base = path.join(
		process.cwd(),
		'packages',
		'vite-plugin-svelte-native',
		'tmp',
		'parity',
		name
	);
	await fs.mkdir(base, { recursive: true });
	return fs.mkdtemp(path.join(base, '-'));
}

async function writeFixture(root, fixture) {
	await fs.mkdir(path.join(root, 'src'), { recursive: true });
	await fs.writeFile(
		path.join(root, 'index.html'),
		'<!doctype html><html><body><div id="app"></div><script type="module" src="/src/main.js"></script></body></html>'
	);

	for (const [relativePath, contents] of Object.entries(fixture.files)) {
		const filePath = path.join(root, relativePath);
		await fs.mkdir(path.dirname(filePath), { recursive: true });
		await fs.writeFile(filePath, contents);
	}
}

async function buildFixture(pluginFactory, root, fixture, pluginLabel) {
	const buildResult = await build({
		root,
		logLevel: 'silent',
		plugins: [
			...pluginFactory({
				configFile: false,
				inspector: false,
				...fixture.options
			})
		],
		build: {
			write: false,
			sourcemap: true,
			rollupOptions: {
				output: {
					entryFileNames: 'assets/[name].js',
					chunkFileNames: 'assets/[name].js',
					assetFileNames: 'assets/[name][extname]'
				}
			}
		}
	});

	const output = Array.isArray(buildResult)
		? buildResult.flatMap((result) => result.output)
		: buildResult.output;

	return normalizeBuildOutput(output);
}

function findPlugin(plugins, name) {
	const plugin = plugins.find((candidate) => candidate.name === name);
	assert.ok(plugin, `expected plugin '${name}' to exist`);
	return plugin;
}

async function runHook(hook, ...args) {
	if (typeof hook === 'function') {
		return hook(...args);
	}
	if (hook && typeof hook === 'object' && typeof hook.handler === 'function') {
		return hook.handler(...args);
	}
}

async function resolvePlugins(
	pluginFactory,
	root,
	inlineOptions = {},
	{ command = 'serve', mode = 'development', base = '/' } = {}
) {
	const plugins = pluginFactory({
		configFile: false,
		inspector: false,
		...inlineOptions
	});

	let userConfig = {
		root,
		base,
		logLevel: 'silent',
		cacheDir: path.join(root, '.vite'),
		server: { hmr: true },
		build: { watch: false },
		resolve: {
			mainFields: [],
			conditions: []
		},
		optimizeDeps: {
			exclude: [],
			include: [],
			rolldownOptions: {
				plugins: []
			}
		},
		experimental: {}
	};
	const configEnv = { command, mode };

	for (const plugin of plugins) {
		const extraConfig = await runHook(plugin.config, userConfig, configEnv);
		if (extraConfig) {
			userConfig = mergeConfig(userConfig, extraConfig);
		}
	}

	const resolvedConfig = {
		...userConfig,
		root,
		base,
		command,
		mode,
		logLevel: 'silent',
		isProduction: mode === 'production',
		cacheDir: path.join(root, '.vite'),
		plugins,
		inlineConfig: userConfig,
		server: {
			hmr: true,
			...(userConfig.server ?? {})
		},
		optimizeDeps: {
			exclude: [],
			include: [],
			rolldownOptions: {
				plugins: []
			},
			...(userConfig.optimizeDeps ?? {})
		},
		resolve: {
			mainFields: [],
			conditions: [],
			...(userConfig.resolve ?? {})
		},
		experimental: userConfig.experimental ?? {}
	};

	const configurePlugin = findPlugin(plugins, 'vite-plugin-svelte:config');
	await runHook(configurePlugin.configResolved, resolvedConfig);
	for (const plugin of plugins) {
		if (plugin === configurePlugin) {
			continue;
		}
		await runHook(plugin.configResolved, resolvedConfig);
	}

	return { plugins, resolvedConfig };
}

function createTransformContext(root, consumer = 'client') {
	return {
		environment: {
			config: {
				root,
				consumer,
				isProduction: false,
				build: { watch: false },
				experimental: { hmrPartialAccept: true }
			},
			transformRequest: async () => ({})
		},
		addWatchFile() {},
		getCombinedSourcemap() {
			return null;
		},
		warn(value) {
			throw new Error(`unexpected warning: ${JSON.stringify(value)}`);
		}
	};
}

function normalizeRollupError(error) {
	return {
		name: error.name,
		id: error.id ?? null,
		code: error.code ?? null,
		message: String(error.message).replace(/\r\n/g, '\n'),
		frame: String(error.frame ?? '').replace(/\r\n/g, '\n'),
		loc: error.loc
			? {
					line: error.loc.line,
					column: error.loc.column,
					file: error.loc.file
				}
			: null
	};
}

function normalizePreprocessMap(map) {
	if (!map) {
		return null;
	}

	return {
		file: map.file ?? null,
		sources: Array.isArray(map.sources) ? [...map.sources] : [],
		names: Array.isArray(map.names) ? [...map.names] : [],
		mappings: typeof map.mappings === 'string' ? map.mappings : null
	};
}

function normalizeWarning(warning) {
	return {
		code: warning.code,
		message: warning.message,
		filename: warning.filename ?? null,
		start: warning.start
			? {
					line: warning.start.line,
					column: warning.start.column
				}
			: null,
		end: warning.end
			? {
					line: warning.end.line,
					column: warning.end.column
				}
			: null
	};
}

function normalizePreprocessResult(result) {
	return {
		code: result?.code?.replace(/\r\n/g, '\n') ?? null,
		dependencies: [...(result?.dependencies ?? [])].sort(),
		map: normalizePreprocessMap(result?.map)
	};
}

function normalizeHotUpdateModules(modules) {
	return (modules ?? [])
		.map((module) => module.url ?? module.id ?? null)
		.filter((value) => value != null)
		.sort();
}

async function collectWarnings(pluginFactory, root, code) {
	const warnings = [];
	const { plugins } = await resolvePlugins(pluginFactory, root, {
		onwarn(warning) {
			warnings.push(normalizeWarning(warning));
		}
	});
	const compilePlugin = findPlugin(plugins, 'vite-plugin-svelte:compile');
	await compilePlugin.transform.handler.call(createTransformContext(root), code, '/src/App.svelte');
	return warnings;
}

async function collectCompileError(pluginFactory, root, code) {
	const { plugins } = await resolvePlugins(pluginFactory, root);
	const compilePlugin = findPlugin(plugins, 'vite-plugin-svelte:compile');
	try {
		await compilePlugin.transform.handler.call(createTransformContext(root), code, '/src/App.svelte');
		throw new Error('expected compile to throw');
	} catch (error) {
		return normalizeRollupError(error);
	}
}

async function runHotUpdateScenario(pluginFactory, root) {
	await writeFixture(root, {
		files: {
			'src/main.js': "import App from './App.svelte';\nconsole.log(App);\n",
			'src/App.svelte': '<style>h1 { color: red; }</style><h1>Hello</h1>'
		}
	});

	const server = await createServer({
		root,
		logLevel: 'silent',
		configFile: false,
		server: {
			middlewareMode: true,
			hmr: true
		},
		plugins: [
			...pluginFactory({
				configFile: false,
				inspector: false,
				compilerOptions: {
					hmr: true
				}
			})
		]
	});

	try {
		const hotUpdatePlugin = findPlugin(server.config.plugins, 'vite-plugin-svelte:hot-update');
		const componentFile = path.join(root, 'src', 'App.svelte').replace(/\\/g, '/');
		const componentUrl = '/src/App.svelte';
		const styleUrl = '/src/App.svelte?svelte&type=style&lang.css';

		await server.transformRequest('/src/main.js');
		await server.transformRequest(componentUrl);
		await server.transformRequest(styleUrl);

		const changedModules = [
			...(server.moduleGraph.getModulesByFile(componentFile) ?? [])
		];

		await fs.writeFile(
			path.join(root, 'src', 'App.svelte'),
			'<style>h1 { color: blue; }</style><h1>Hello world</h1>'
		);

		const changed = await hotUpdatePlugin.hotUpdate.handler.call(
			{ environment: server.environments.client },
			{
				file: componentFile,
				modules: changedModules,
				timestamp: Date.now()
			}
		);

		const unchangedModules = [
			...(server.moduleGraph.getModulesByFile(componentFile) ?? [])
		];

		await fs.writeFile(
			path.join(root, 'src', 'App.svelte'),
			'<style>h1 { color: blue; }</style><h1>Hello world</h1>'
		);

		const unchanged = await hotUpdatePlugin.hotUpdate.handler.call(
			{ environment: server.environments.client },
			{
				file: componentFile,
				modules: unchangedModules,
				timestamp: Date.now()
			}
		);

		return {
			appliesToClient: hotUpdatePlugin.applyToEnvironment({ config: { consumer: 'client' } }),
			appliesToServer: hotUpdatePlugin.applyToEnvironment({ config: { consumer: 'server' } }),
			changed: normalizeHotUpdateModules(changed),
			unchanged: normalizeHotUpdateModules(unchanged)
		};
	} finally {
		await Promise.all(
			Object.values(server.environments).map(async (environment) => {
				await environment.waitForRequestsIdle?.();
				await environment.close?.();
			})
		);
		await server.ws.close();
		await server.watcher.close();
	}
}

async function runInspectorScenario(pluginFactory, root) {
	const { plugins } = await resolvePlugins(
		pluginFactory,
		root,
		{ inspector: true },
		{ base: '/base/' }
	);
	const inspectorPlugin = findPlugin(plugins, 'vite-plugin-svelte-inspector');
	return {
		appliesToClient: inspectorPlugin.applyToEnvironment({ config: { consumer: 'client' } }),
		appliesToServer: inspectorPlugin.applyToEnvironment({ config: { consumer: 'server' } }),
		optionsId: await inspectorPlugin.resolveId.handler('virtual:svelte-inspector-options'),
		optionsModule: await inspectorPlugin.load.handler('virtual:svelte-inspector-options'),
		clientTransform: await inspectorPlugin.transform.handler(
			'export const client = true;',
			'/node_modules/vite/dist/client/client.mjs'
		)
	};
}

async function runOptimizerScenario(pluginFactory, root) {
	const { plugins, resolvedConfig } = await resolvePlugins(pluginFactory, root, {
		prebundleSvelteLibraries: true
	});
	const optimizerPlugin = findPlugin(plugins, 'vite-plugin-svelte:setup-optimizer');
	const optimizePlugin = resolvedConfig.optimizeDeps.rolldownOptions.plugins.find(
		(plugin) => plugin.name === 'vite-plugin-svelte:optimize'
	);
	const optimizeModulePlugin = resolvedConfig.optimizeDeps.rolldownOptions.plugins.find(
		(plugin) => plugin.name === 'vite-plugin-svelte:optimize-module'
	);

	assert.ok(optimizePlugin, 'expected optimize plugin to exist');
	assert.ok(optimizeModulePlugin, 'expected optimize module plugin to exist');

	optimizePlugin.options({ plugins: [{ name: 'not-scanner' }] });
	optimizeModulePlugin.options({ plugins: [{ name: 'not-scanner' }] });

	return {
		configShape: {
			extensions: [...(resolvedConfig.optimizeDeps.extensions ?? [])],
			rolldownPluginNames: resolvedConfig.optimizeDeps.rolldownOptions.plugins.map(
				(plugin) => plugin.name
			),
			optimizeHasTransform: typeof optimizePlugin.transform?.handler === 'function',
			optimizeHasBuildStart: typeof optimizePlugin.buildStart === 'function',
			optimizeHasBuildEnd: typeof optimizePlugin.buildEnd === 'function',
			optimizeModuleHasTransform: typeof optimizeModulePlugin.transform?.handler === 'function',
			optimizeModuleHasBuildStart: typeof optimizeModulePlugin.buildStart === 'function',
			optimizeModuleHasBuildEnd: typeof optimizeModulePlugin.buildEnd === 'function'
		}
	};
}

function normalizeBuildOutput(output) {
	const files = output
		.map((entry) => {
			if (entry.type === 'chunk') {
				return {
					fileName: entry.fileName,
					type: entry.type,
					code: normalizeText(entry.code),
					map: normalizeMap(entry.map)
				};
			}

			return {
				fileName: entry.fileName,
				type: entry.type,
				source: normalizeText(String(entry.source)),
				map: normalizeMap(entry.map ?? null)
			};
		})
		.sort((left, right) => left.fileName.localeCompare(right.fileName));

	return files;
}

function normalizeText(value) {
	return value.replace(/\r\n/g, '\n').trim();
}

function normalizeMap(map) {
	if (!map) {
		return null;
	}

	return {
		version: map.version,
		file: map.file ?? null,
		sources: [...map.sources],
		names: [...map.names],
		mappings: map.mappings
	};
}

function mapShape(map) {
	if (!map) {
		return null;
	}

	return {
		version: map.version,
		file: map.file,
		sources: map.sources,
		names: map.names
	};
}

function isSourcemapAsset(entry) {
	return entry.type === 'asset' && entry.fileName.endsWith('.map');
}

const fixtures = {
	component: {
		files: {
			'src/main.js': "import App from './App.svelte';\nconsole.log(App);\n",
			'src/App.svelte': '<style>h1 { color: red; }</style><h1>Hello</h1>'
		}
	},
	moduleTs: {
		files: {
			'src/main.js':
				"import { attachment } from './attachment.svelte.ts';\nconsole.log(attachment.value);\n",
			'src/attachment.svelte.ts':
				'interface Attachment { value: number }\nexport const attachment: Attachment = { value: 42 };\n'
		}
	},
	preprocessAndCssHash: {
		options: {
			preprocess: {
				async markup({ content }) {
					return {
						code: content.replace('__NAME__', 'world')
					};
				}
			},
			compilerOptions: {
				cssHash() {
					return 'native-hash';
				}
			}
		},
		files: {
			'src/main.js': "import App from './App.svelte';\nconsole.log(App);\n",
			'src/App.svelte': '<style>h1 { color: red; }</style><h1>Hello __NAME__</h1>'
		}
	}
};

test('native plugin matches upstream plugin build output on curated fixtures', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();

	for (const [fixtureName, fixture] of Object.entries(fixtures)) {
		const root = await createParityTempDir(fixtureName);
		await writeFixture(root, fixture);

		const [upstreamOutput, nativeOutput] = await Promise.all([
			buildFixture(upstreamSvelte, root, fixture, 'upstream'),
			buildFixture(nativeSvelte, root, fixture, 'native')
		]);

		assert.deepEqual(
			nativeOutput
				.filter((entry) => !isSourcemapAsset(entry))
				.map(({ map, ...entry }) => entry),
			upstreamOutput
				.filter((entry) => !isSourcemapAsset(entry))
				.map(({ map, ...entry }) => entry),
			`expected native plugin to match upstream output for fixture '${fixtureName}'`
		);

		// The emitted `.map` asset mirrors the chunk sourcemap payload, so compare sourcemap
		// structure from the chunk/output map directly instead of asserting the duplicated JSON
		// asset byte-for-byte here.
		assert.deepEqual(
			nativeOutput.map((entry) => ({ fileName: entry.fileName, map: mapShape(entry.map) })),
			upstreamOutput.map((entry) => ({ fileName: entry.fileName, map: mapShape(entry.map) })),
			`expected native plugin to match upstream sourcemap structure for fixture '${fixtureName}'`
		);

		for (let i = 0; i < nativeOutput.length; i += 1) {
			const nativeMap = nativeOutput[i].map;
			const upstreamMap = upstreamOutput[i].map;
			assert.equal(
				Boolean(nativeMap),
				Boolean(upstreamMap),
				`expected native plugin to preserve sourcemap presence for fixture '${fixtureName}' output '${nativeOutput[i].fileName}'`
			);
			if (!nativeMap || !upstreamMap) {
				continue;
			}
			assert.equal(
				typeof nativeMap.mappings,
				'string',
				`expected native plugin to emit string sourcemap mappings for fixture '${fixtureName}' output '${nativeOutput[i].fileName}'`
			);
			assert.equal(
				nativeMap.mappings.length > 0,
				upstreamMap.mappings.length > 0,
				`expected native plugin to preserve sourcemap mapping presence for fixture '${fixtureName}' output '${nativeOutput[i].fileName}'`
			);
		}
	}
});

test('native plugin matches upstream warning callback payloads', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();
	const root = await createParityTempDir('warnings');
	const code = '<img src="photo.jpg">';

	const [upstreamWarnings, nativeWarnings] = await Promise.all([
		collectWarnings(upstreamSvelte, root, code),
		collectWarnings(nativeSvelte, root, code)
	]);

	assert.deepEqual(nativeWarnings, upstreamWarnings);
});

test('native plugin matches upstream compile error shaping', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();
	const root = await createParityTempDir('errors');
	const code = '<svelte:head foo="bar"></svelte:head>';

	const [upstreamError, nativeError] = await Promise.all([
		collectCompileError(upstreamSvelte, root, code),
		collectCompileError(nativeSvelte, root, code)
	]);

	assert.deepEqual(nativeError, upstreamError);
});

test('native plugin matches upstream hot-update behavior', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();
	const upstreamRoot = await createParityTempDir('hot-update-upstream');
	const nativeRoot = await createParityTempDir('hot-update-native');

	const upstreamResult = await runHotUpdateScenario(upstreamSvelte, upstreamRoot);
	const nativeResult = await runHotUpdateScenario(nativeSvelte, nativeRoot);

	assert.deepEqual(nativeResult, upstreamResult);
});

test('native plugin matches upstream inspector hooks', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();
	const root = await createParityTempDir('inspector');

	const [upstreamResult, nativeResult] = await Promise.all([
		runInspectorScenario(upstreamSvelte, root),
		runInspectorScenario(nativeSvelte, root)
	]);

	assert.deepEqual(nativeResult, upstreamResult);
});

test('native plugin matches upstream optimizer behavior', async () => {
	const upstreamSvelte = await loadUpstreamSvelte();
	const root = await createParityTempDir('optimizer');

	const [upstreamResult, nativeResult] = await Promise.all([
		runOptimizerScenario(upstreamSvelte, root),
		runOptimizerScenario(nativeSvelte, root)
	]);

	assert.deepEqual(nativeResult, upstreamResult);
});

test('native vitePreprocess matches upstream helper output', async () => {
	const upstream = await loadUpstreamModule();
	const root = await createParityTempDir('vite-preprocess');
	const filename = path.join(root, 'src', 'App.svelte');
	await fs.mkdir(path.dirname(filename), { recursive: true });
	await fs.writeFile(path.join(root, 'src', 'dep.css'), 'h1 { color: red; }');

	const upstreamPreprocess = upstream.vitePreprocess({ script: true });
	const nativePreprocess = nativeVitePreprocess({ script: true });

	const [upstreamScript, nativeScript] = await Promise.all([
		upstreamPreprocess.script({
			attributes: { lang: 'ts' },
			content: 'export const value: number = 42;',
			filename
		}),
		nativePreprocess.script({
			attributes: { lang: 'ts' },
			content: 'export const value: number = 42;',
			filename
		})
	]);

	assert.deepEqual(normalizePreprocessResult(nativeScript), normalizePreprocessResult(upstreamScript));

	const [upstreamStyle, nativeStyle] = await Promise.all([
		upstreamPreprocess.style({
			attributes: { lang: 'css' },
			content: '@import "./dep.css";\nh1 { color: blue; }',
			filename
		}),
		nativePreprocess.style({
			attributes: { lang: 'css' },
			content: '@import "./dep.css";\nh1 { color: blue; }',
			filename
		})
	]);

	assert.deepEqual(normalizePreprocessResult(nativeStyle), normalizePreprocessResult(upstreamStyle));
});
