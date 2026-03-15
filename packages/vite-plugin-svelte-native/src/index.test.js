import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { build } from 'vite';

import { loadSvelteConfig, svelte, vitePreprocess } from './index.js';

function findPlugin(plugins, name) {
	const plugin = plugins.find((candidate) => candidate.name === name);
	assert.ok(plugin, `expected plugin '${name}' to exist`);
	return plugin;
}

async function resolvePlugins(plugins, { root = process.cwd(), command = 'serve' } = {}) {
	const configurePlugin = findPlugin(plugins, 'vite-plugin-svelte:config');
	const configHook = configurePlugin.config;
	if (configHook && typeof configHook === 'object' && typeof configHook.handler === 'function') {
		await configHook.handler({ root }, { command, mode: 'development' });
	}

	const resolvedConfig = {
		root,
		isProduction: false,
		command,
		mode: 'development',
		logLevel: 'info',
		cacheDir: path.join(root, '.vite'),
		plugins,
		server: {
			hmr: true
		},
		optimizeDeps: {
			exclude: [],
			include: [],
			rolldownOptions: {
				plugins: []
			}
		},
		resolve: {
			mainFields: [],
			conditions: []
		},
		experimental: {}
	};

	const configResolvedHook = configurePlugin.configResolved;
	if (
		configResolvedHook &&
		typeof configResolvedHook === 'object' &&
		typeof configResolvedHook.handler === 'function'
	) {
		await configResolvedHook.handler(resolvedConfig);
	}

	for (const plugin of plugins) {
		if (plugin === configurePlugin) {
			continue;
		}
		if (typeof plugin.configResolved === 'function') {
			await plugin.configResolved(resolvedConfig);
		} else if (
			plugin.configResolved &&
			typeof plugin.configResolved === 'object' &&
			typeof plugin.configResolved.handler === 'function'
		) {
			await plugin.configResolved.handler(resolvedConfig);
		}
	}

	return { configurePlugin, resolvedConfig };
}

function createTransformContext(root, consumer = 'client') {
	return {
		environment: {
			config: {
				root,
				consumer,
				isProduction: false,
				build: {
					watch: false
				},
				experimental: {
					hmrPartialAccept: true
				}
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

async function createPackageTempDir(prefix) {
	const base = path.join(
		process.cwd(),
		'packages',
		'vite-plugin-svelte-native',
		'tmp',
		prefix
	);
	await fs.mkdir(base, { recursive: true });
	return fs.mkdtemp(path.join(base, '-'));
}

test('svelte returns the upstream plugin pipeline shape', () => {
	const plugins = svelte({ configFile: false, inspector: false });
	assert.deepEqual(
		plugins.map((plugin) => plugin.name),
		[
			'vite-plugin-svelte',
			'vite-plugin-svelte:config',
			'vite-plugin-svelte:setup-optimizer',
			'vite-plugin-svelte:load-compiled-css',
			'vite-plugin-svelte:load-custom',
			'vite-plugin-svelte:preprocess',
			'vite-plugin-svelte:compile',
			'vite-plugin-svelte:compile-module',
			'vite-plugin-svelte:hot-update',
			'vite-plugin-svelte-inspector'
		]
	);
});

test('loadSvelteConfig reads svelte.config.mjs and ignores cjs by default', async () => {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), 'vite-plugin-svelte-native-config-'));
	await fs.writeFile(
		path.join(root, 'svelte.config.mjs'),
		`export default { compilerOptions: { dev: true, preserveWhitespace: true } }`
	);
	await fs.writeFile(
		path.join(root, 'svelte.config.cjs'),
		`module.exports = { compilerOptions: { dev: false } }`
	);

	const loaded = await loadSvelteConfig({ root }, {});
	assert.equal(loaded.compilerOptions.dev, true);
	assert.equal(loaded.compilerOptions.preserveWhitespace, true);
	assert.ok(String(loaded.configFile).endsWith('svelte.config.mjs'));
});

test('compile plugin transforms a component and exposes compiled css as a virtual module', async () => {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), 'vite-plugin-svelte-native-compile-'));
	const plugins = svelte({ configFile: false, inspector: false });
	await resolvePlugins(plugins, { root });

	const compilePlugin = findPlugin(plugins, 'vite-plugin-svelte:compile');
	const loadCssPlugin = findPlugin(plugins, 'vite-plugin-svelte:load-compiled-css');
	const transformed = await compilePlugin.transform.handler.call(
		createTransformContext(root),
		'<style>h1 { color: red; }</style><h1>Hello</h1>',
		'/src/App.svelte'
	);

	assert.match(transformed.code, /import "\/src\/App\.svelte\?svelte&type=style&lang\.css";/);
	assert.equal(transformed.moduleType, 'js');

	const loadedCss = await loadCssPlugin.load.handler.call(
		{
			...createTransformContext(root),
			getModuleInfo() {
				return {
					meta: transformed.meta
				};
			}
		},
		'/src/App.svelte?svelte&type=style&lang.css'
	);
	assert.equal(loadedCss.code.includes('color: red'), true);
	assert.equal(loadedCss.moduleType, 'css');
});

test('compile plugin forwards cssHash callbacks through the native bridge', async () => {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), 'vite-plugin-svelte-native-csshash-'));
	const plugins = svelte({
		configFile: false,
		inspector: false,
		compilerOptions: {
			cssHash() {
				return 'native-hash';
			}
		}
	});
	await resolvePlugins(plugins, { root });

	const compilePlugin = findPlugin(plugins, 'vite-plugin-svelte:compile');
	const loadCssPlugin = findPlugin(plugins, 'vite-plugin-svelte:load-compiled-css');
	const transformed = await compilePlugin.transform.handler.call(
		createTransformContext(root),
		'<style>h1 { color: red; }</style><h1>Hello</h1>',
		'/src/App.svelte'
	);
	const loadedCss = await loadCssPlugin.load.handler.call(
		{
			...createTransformContext(root),
			getModuleInfo() {
				return {
					meta: transformed.meta
				};
			}
		},
		'/src/App.svelte?svelte&type=style&lang.css'
	);

	assert.match(loadedCss.code, /native-hash/);
});

test('compile module plugin accepts .svelte.ts inputs through the native bridge', async () => {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), 'vite-plugin-svelte-native-module-'));
	const plugins = svelte({ configFile: false, inspector: false });
	await resolvePlugins(plugins, { root });

	const compileModulePlugin = findPlugin(plugins, 'vite-plugin-svelte:compile-module');
	const transformed = await compileModulePlugin.transform.handler.call(
		createTransformContext(root),
		'interface Attachment { value: number }\nexport const attachment: Attachment = { value: 42 };',
		'/src/attachment.svelte.ts'
	);

	assert.equal(transformed.moduleType, 'js');
	assert.match(transformed.code, /export const attachment/);
	assert.doesNotMatch(transformed.code, /interface Attachment/);
});

test('preprocess plugin runs async preprocessors without svelte/compiler', async () => {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), 'vite-plugin-svelte-native-preprocess-'));
	const plugins = svelte({
		configFile: false,
		inspector: false,
		preprocess: {
			async markup({ content, filename }) {
				assert.equal(filename, '/src/App.svelte');
				return {
					code: content.replace('__NAME__', 'world'),
					dependencies: ['/virtual/dependency.txt']
				};
			}
		}
	});
	await resolvePlugins(plugins, { root });

	const preprocessPlugin = findPlugin(plugins, 'vite-plugin-svelte:preprocess');
	const preprocessed = await preprocessPlugin.transform.handler.call(
		createTransformContext(root),
		'<h1>Hello __NAME__</h1>',
		'/src/App.svelte'
	);

	assert.equal(preprocessed.code, '<h1>Hello world</h1>');
});

test('vite build works end to end with preprocess and native compiler callbacks', async () => {
	const root = await createPackageTempDir('build');
	await fs.mkdir(path.join(root, 'src'), { recursive: true });
	await fs.writeFile(
		path.join(root, 'index.html'),
		'<!doctype html><html><body><div id="app"></div><script type="module" src="/src/main.js"></script></body></html>'
	);
	await fs.writeFile(
		path.join(root, 'src', 'main.js'),
		"import App from './App.svelte';\nconsole.log(App);\n"
	);
	await fs.writeFile(
		path.join(root, 'src', 'App.svelte'),
		'<style>h1 { color: red; }</style><h1>Hello __NAME__</h1>'
	);

	const outputs = await build({
		root,
		logLevel: 'silent',
		plugins: [
			...svelte({
				configFile: false,
				inspector: false,
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
			})
		],
		build: {
			write: false
		}
	});

	const output = Array.isArray(outputs)
		? outputs.flatMap((result) => result.output)
		: outputs.output;
	const jsAsset = output.find((entry) => entry.type === 'chunk');
	const cssAsset = output.find((entry) => entry.type === 'asset' && entry.fileName.endsWith('.css'));

	assert.ok(jsAsset, 'expected a built JavaScript chunk');
	assert.ok(cssAsset, 'expected a built CSS asset');
	assert.match(String(jsAsset.code), /Hello world/);
	assert.match(String(cssAsset.source), /native-hash/);
});

test('vitePreprocess stays available as the upstream helper export', () => {
	const preprocess = vitePreprocess();
	assert.equal(preprocess.name, 'vite-preprocess');
	assert.equal(typeof preprocess.script, 'undefined');
	assert.equal(typeof preprocess.style, 'function');
});
