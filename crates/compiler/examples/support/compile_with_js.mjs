import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import fs from 'node:fs/promises';

const rawRequest = process.argv[2];

if (!rawRequest) {
	throw new Error('expected serialized request JSON as the first argument');
}

const request = JSON.parse(rawRequest);
const packageRoot = path.join(request.repoRoot, 'packages', 'svelte');
const packageJson = JSON.parse(await fs.readFile(path.join(packageRoot, 'package.json'), 'utf8'));
const compilerModule = await import(
	pathToFileURL(path.join(packageRoot, 'src', 'compiler', 'index.js')).href
);

const compile =
	request.kind === 'module' ? compilerModule.compileModule : compilerModule.compile;
const result = compile(request.source, request.options);

const response = {
	metadata: {
		generatedAt: new Date().toISOString(),
		jsPackageVersion: packageJson.version,
		nodeVersion: process.version
	},
	output: {
		js: {
			code: result.js.code
		},
		css: result.css
			? {
					code: result.css.code,
					hasGlobal: result.css.hasGlobal
				}
			: null,
		warnings: result.warnings.map((warning) => ({
			code: warning.code,
			message: warning.message
		})),
		metadata: {
			runes: result.metadata.runes
		}
	}
};

process.stdout.write(JSON.stringify(response));
