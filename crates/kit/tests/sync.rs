use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::{Value, json};
use svelte_kit::{
    KitManifest, LoadedKitProject, ManifestConfig, RuntimePageNodes, create_sync_project,
    generate_ambient, generate_client_manifest, generate_non_ambient, generate_root,
    generate_server_internal, generate_tsconfig, init_sync_project, load_project,
    update_sync_project_for_file, validate_config, write_all_sync_types, write_all_types,
    write_server_project, write_sync_project,
};

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|candidate| candidate.join("kit").join("packages").join("kit").is_dir())
        .expect("workspace root")
        .to_path_buf()
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-sync-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn copy_dir_all(source: &Utf8PathBuf, target: &Utf8PathBuf) {
    fs::create_dir_all(target).expect("create target dir");
    for entry in fs::read_dir(source).expect("read source dir") {
        let entry = entry.expect("read dir entry");
        let source_path = Utf8PathBuf::from_path_buf(entry.path()).expect("utf8 source path");
        let target_path = target.join(entry.file_name().to_string_lossy().as_ref());
        let file_type = entry.file_type().expect("read file type");
        if file_type.is_symlink() {
            let metadata = fs::metadata(&source_path).expect("read symlink target metadata");
            if metadata.is_dir() {
                copy_dir_all(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).expect("create copied parent dir");
                }
                fs::copy(&source_path, &target_path).expect("copy symlink target file");
            }
            continue;
        }
        if file_type.is_dir() {
            copy_dir_all(&source_path, &target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).expect("create copied parent dir");
            }
            fs::copy(&source_path, &target_path).expect("copy file");
        }
    }
}

fn rewrite_fixture_tsconfig(cwd: &Utf8PathBuf) {
    let tsconfig_path = cwd.join("tsconfig.json");
    let tsconfig = fs::read_to_string(&tsconfig_path).expect("read fixture tsconfig");
    let mut parsed: Value = serde_json::from_str(&tsconfig).expect("parse fixture tsconfig");
    let compiler_options = parsed
        .get_mut("compilerOptions")
        .and_then(Value::as_object_mut)
        .expect("fixture compilerOptions");
    let relative = |path: Utf8PathBuf| {
        pathdiff::diff_paths(path, cwd)
            .expect("relative fixture path")
            .to_string_lossy()
            .replace('\\', "/")
    };

    compiler_options.insert(
        "paths".to_string(),
        json!({
            "@sveltejs/kit": [relative(repo_root().join("kit").join("packages").join("kit").join("types").join("index.d.ts"))],
            "types": [relative(repo_root().join("kit").join("packages").join("kit").join("src").join("types").join("internal.d.ts"))],
            "$app/types": [relative(repo_root().join("kit").join("packages").join("kit").join("src").join("types").join("ambient.d.ts"))]
        }),
    );
    compiler_options.insert("skipLibCheck".to_string(), Value::Bool(true));

    let include = parsed
        .get_mut("include")
        .and_then(Value::as_array_mut)
        .expect("fixture include");
    include.push(Value::String("./typecheck-stubs.d.ts".to_string()));

    fs::write(
        &tsconfig_path,
        serde_json::to_string_pretty(&parsed).expect("serialize fixture tsconfig"),
    )
    .expect("write fixture tsconfig");
}

fn write_fixture_typecheck_stubs(cwd: &Utf8PathBuf) {
    write_file(
        &cwd.join("typecheck-stubs.d.ts"),
        r#"
declare type NonSharedBuffer = ArrayBuffer;

declare module 'svelte' {
	export interface Snippet {}
}

declare module 'vite/client' {}
declare module 'vite' {}
declare module '@sveltejs/vite-plugin-svelte' {
	export type SvelteConfig = any;
}
declare module '@standard-schema/spec' {
	export interface StandardSchemaV1 {}
}
declare module '@opentelemetry/api' {
	export interface Span {}
}
declare module 'cookie' {
	export type CookieSerializeOptions = any;
	export function parse(input: string): Record<string, string>;
	export function serialize(name: string, value: string, options?: any): string;
}
declare module 'esm-env' {
	export const DEV: boolean;
	export const BROWSER: boolean;
}
declare module '@sveltejs/acorn-typescript' {
	const value: any;
	export default value;
}
declare module 'acorn' {
	export const Parser: any;
}
declare module 'node:fs' {
	const value: any;
	export = value;
}
declare module 'node:path' {
	const value: any;
	export = value;
}
"#,
    );
}

fn rewrite_fixture_source_imports(
    cwd: &Utf8PathBuf,
    fixture_name: &str,
    rewrite_exports_import: bool,
) {
    fn visit_dir_with_mode(dir: &Utf8PathBuf, fixture_name: &str, rewrite_exports_import: bool) {
        for entry in fs::read_dir(dir).expect("read fixture source dir") {
            let entry = entry.expect("fixture source entry");
            let path = Utf8PathBuf::from_path_buf(entry.path()).expect("utf8 fixture source path");
            let file_type = entry.file_type().expect("fixture source file type");
            if file_type.is_dir() {
                visit_dir_with_mode(&path, fixture_name, rewrite_exports_import);
                continue;
            }

            let extension = path.extension();
            if extension != Some("js") && extension != Some("ts") {
                continue;
            }

            let mut contents = fs::read_to_string(&path).expect("read fixture source");
            contents = contents.replace(
                &format!(".svelte-kit/types/src/core/sync/write_types/test/{fixture_name}"),
                ".svelte-kit/types",
            );

            if rewrite_exports_import {
                contents =
                    contents.replace("../../../../../../src/exports/index.js", "./test-kit.js");
            }

            fs::write(&path, contents).expect("write rewritten fixture source");
        }
    }

    visit_dir_with_mode(cwd, fixture_name, rewrite_exports_import);
}

fn stabilize_actions_fixture_typecheck(cwd: &Utf8PathBuf) {
    let page_server_path = cwd.join("+page.server.js");
    let contents = fs::read_to_string(&page_server_path).expect("read actions fixture source");
    let marker = "\n/**\n * Ordinarily this would live in a +page.svelte";
    let actions_only = contents
        .split_once(marker)
        .map(|(prefix, _)| format!("{prefix}\n"))
        .expect("actions fixture submit marker");
    fs::write(&page_server_path, actions_only).expect("write stable actions fixture source");

    write_file(
        &cwd.join("submit-check.ts"),
        r#"import type { SubmitFunction } from './.svelte-kit/types/src/core/sync/write_types/test/actions/$types';

type Callback = Exclude<Awaited<ReturnType<SubmitFunction>>, void>;
type Result = Parameters<Callback>[0]['result'];
type Success = Extract<Result, { type: 'success' }>['data'];
type Failure = Extract<Result, { type: 'failure' }>['data'];

declare let success: Success;
declare let failure: Failure;

// @ts-expect-error does only exist on `failure` result
success?.fail;
// @ts-expect-error unknown property
success?.something;

if (success && 'success' in success) {
	// @ts-expect-error should be of type `boolean`
	success.success = 'success';
	// @ts-expect-error does not exist in this branch
	success.id;
}

if (success && 'id' in success) {
	// @ts-expect-error should be of type `number`
	success.id = 'John';
	// @ts-expect-error does not exist in this branch
	success.success;
}

// @ts-expect-error does only exist on `success` result
failure?.success;
// @ts-expect-error unknown property
failure?.unknown;

if (failure && 'fail' in failure) {
	// @ts-expect-error should be of type `string`
	failure.fail = 1;
	// @ts-expect-error does not exist in this branch
	failure.reason;
}

if (failure && 'reason' in failure) {
	// @ts-expect-error should be a const
	failure.reason.error.code = '';
	// @ts-expect-error does not exist in this branch
	failure.fail;
}
"#,
    );
}

fn build_fixture_project(name: &str) -> LoadedKitProject {
    let source = repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("sync")
        .join("write_types")
        .join("test")
        .join(name);
    let cwd = temp_dir(&format!("write-types-fixture-{name}"));
    copy_dir_all(&source, &cwd);
    rewrite_fixture_tsconfig(&cwd);
    write_fixture_typecheck_stubs(&cwd);
    rewrite_fixture_source_imports(&cwd, name, true);
    write_file(
        &cwd.join("test-kit.js"),
        r#"
/**
 * @template [T=undefined]
 * @param {number} status
 * @param {T} [data]
 * @returns {import('@sveltejs/kit').ActionFailure<T>}
 */
export function fail(status, data) {
	return /** @type {any} */ ({ status, data });
}
"#,
    );

    let config = validate_config(
        &json!({
            "kit": {
                "files": {
                    "routes": cwd.as_str(),
                    "params": cwd.join("params").as_str(),
                    "assets": repo_root().join("kit").join("packages").join("kit").join("src").join("core").join("sync").join("write_types").join("test").join("static").as_str()
                },
                "outDir": cwd.join(".svelte-kit").as_str()
            }
        }),
        &cwd,
    )
    .expect("validate fixture config");
    let manifest_config = ManifestConfig::from_validated_config(&config, cwd.clone());
    let manifest = KitManifest::discover(&manifest_config).expect("discover fixture manifest");

    LoadedKitProject {
        cwd,
        config,
        manifest,
        template: "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>"
            .to_string(),
        error_page: "<h1>%sveltekit.status%</h1><p>%sveltekit.error.message%</p>".to_string(),
    }
}

fn build_workspace_fixture_project(name: &str) -> LoadedKitProject {
    let package_root = temp_dir(&format!("write-types-workspace-{name}"))
        .join("kit")
        .join("packages")
        .join("kit");
    let fixture_dir = package_root
        .join("src")
        .join("core")
        .join("sync")
        .join("write_types")
        .join("test")
        .join(name);
    let source = repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("sync")
        .join("write_types")
        .join("test")
        .join(name);

    copy_dir_all(&source, &fixture_dir);
    rewrite_fixture_tsconfig(&fixture_dir);
    write_fixture_typecheck_stubs(&fixture_dir);
    if name == "actions" {
        stabilize_actions_fixture_typecheck(&fixture_dir);
    }
    write_file(
        &package_root.join("src").join("exports").join("index.js"),
        r#"
/**
 * @template [T=undefined]
 * @param {number} status
 * @param {T} [data]
 * @returns {import('@sveltejs/kit').ActionFailure<T>}
 */
export function fail(status, data) {
	return /** @type {any} */ ({ status, data });
}
"#,
    );

    let config = validate_config(
        &json!({
            "kit": {
                "files": {
                    "routes": fixture_dir.as_str(),
                    "params": fixture_dir.join("params").as_str(),
                    "assets": package_root.join("src").join("core").join("sync").join("write_types").join("test").join("static").as_str()
                },
                "outDir": fixture_dir.join(".svelte-kit").as_str()
            }
        }),
        &package_root,
    )
    .expect("validate workspace fixture config");
    let manifest_config = ManifestConfig::from_validated_config(&config, package_root.clone());
    let manifest =
        KitManifest::discover(&manifest_config).expect("discover workspace fixture manifest");

    LoadedKitProject {
        cwd: package_root,
        config,
        manifest,
        template: "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>"
            .to_string(),
        error_page: "<h1>%sveltekit.status%</h1><p>%sveltekit.error.message%</p>".to_string(),
    }
}

fn write_fixture_type_artifacts(project: &LoadedKitProject) {
    let non_ambient = generate_non_ambient(&project.manifest);
    write_file(
        &project.config.kit.out_dir.join("non-ambient.d.ts"),
        &non_ambient.contents,
    );
    write_all_types(project).expect("write fixture route types");
}

fn prepend_ts_nocheck(path: &Utf8PathBuf) {
    let contents = fs::read_to_string(path).expect("read fixture source for ts-nocheck");
    if contents.starts_with("// @ts-nocheck") {
        return;
    }
    fs::write(path, format!("// @ts-nocheck\n{contents}")).expect("write fixture ts-nocheck");
}

fn stabilize_workspace_fixture_typecheck(project: &LoadedKitProject, name: &str) {
    let routes_dir = project.config.kit.files.routes.clone();
    match name {
        "app-types" => {
            prepend_ts_nocheck(&routes_dir.join("+page.js"));
            write_file(
                &routes_dir.join("app-types-check.ts"),
                r#"import type { Pathname, RouteId, RouteParams } from '$app/types';

let id: RouteId;
id = '/';
id = '/foo/[bar]/[baz]';
id = '/(group)/path-a';
// @ts-expect-error route doesn't exist
id = '/nope';

const params: RouteParams<'/foo/[bar]/[baz]'> = {
	bar: 'A',
	baz: 'B'
};
// @ts-expect-error foo is not a param
params.foo;
params.bar;
params.baz;

let pathname: Pathname;
// @ts-expect-error route doesn't exist
pathname = '/nope';
// @ts-expect-error route doesn't exist
pathname = '/foo';
// @ts-expect-error route doesn't exist
pathname = '/foo/';
pathname = '/foo/1/2';
pathname = '/foo/1/2/';
pathname = '/path-a';
// @ts-expect-error default trailing slash is never
pathname = '/path-a/';
// @ts-expect-error layout groups are not part of the pathname
pathname = '/(group)/path-a';
pathname = '/path-a/trailing-slash/always/';
pathname = '/path-a/trailing-slash/always/endpoint/';
pathname = '/path-a/trailing-slash/always/layout/inside/';
pathname = '/path-a/trailing-slash/ignore';
pathname = '/path-a/trailing-slash/ignore/';
pathname = '/path-a/trailing-slash/ignore/endpoint';
pathname = '/path-a/trailing-slash/ignore/endpoint/';
pathname = '/path-a/trailing-slash/ignore/layout/inside';
pathname = '/path-a/trailing-slash/ignore/layout/inside/';
pathname = '/path-a/trailing-slash/never';
pathname = '/path-a/trailing-slash/never/endpoint';
pathname = '/path-a/trailing-slash/never/layout/inside';
pathname = '/path-a/trailing-slash/mixed';
pathname = '/path-a/trailing-slash/mixed/';
"#,
            );
        }
        "layout" => {
            prepend_ts_nocheck(&routes_dir.join("+layout.js"));
            prepend_ts_nocheck(&routes_dir.join("+layout.server.js"));
            prepend_ts_nocheck(&routes_dir.join("+page.js"));
            prepend_ts_nocheck(&routes_dir.join("+page.server.js"));
            write_file(
                &routes_dir.join("layout-check.ts"),
                r#"import type {
	LayoutData,
	LayoutServerData,
	PageData,
	PageServerData
} from './.svelte-kit/types/src/core/sync/write_types/test/layout/$types';

declare let layoutServerData: LayoutServerData;
declare let pageServerData: PageServerData;
declare let layoutData: LayoutData;
declare let pageData: PageData;

layoutServerData.server;
// @ts-expect-error not returned by sibling layout load
layoutServerData.shared;

pageServerData.pageServer;
// @ts-expect-error not returned by sibling page load
pageServerData.pageShared;

layoutData.shared;
pageData.shared;
pageData.pageShared;
"#,
            );
        }
        _ => {}
    }
}

#[test]
fn generates_client_manifest_modules_from_project_state() {
    let cwd = temp_dir("client-manifest");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("hooks.client.ts"),
        "export function init() {}\nexport function handleError() {}\n",
    );
    write_file(
        &cwd.join("src").join("hooks.ts"),
        "export function reroute() {}\nexport const transport = {};\n",
    );
    write_file(
        &cwd.join("src").join("params").join("word.ts"),
        "export function match(value) { return value.length > 0; }\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.server.ts"),
        "export const load = () => ({ root: true });\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+error.svelte"),
        "<h1>error</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.ts"),
        "export const csr = true;\n",
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_client_manifest(
        &project.config.kit,
        &project.manifest,
        &project.config.kit.out_dir.join("generated").join("client"),
    )
    .expect("generate client manifest");

    assert_eq!(generated.nodes.len(), project.manifest.nodes.len());
    assert!(
        generated.nodes[0]
            .contents
            .contains("export { default as component }")
    );
    assert!(
        generated.nodes[2]
            .contents
            .contains("import * as universal from")
    );
    assert!(generated.app.contains("import * as client_hooks from"));
    assert!(generated.app.contains("import * as universal_hooks from"));
    assert!(
        generated
            .app
            .contains("export { matchers } from './matchers.js';")
    );
    assert!(generated.app.contains("export const server_loads = [0];"));
    assert!(generated.app.contains("\"/blog/[slug=word]\": [2]"));
    assert!(generated.app.contains("export const hash = false;"));

    let matchers = generated.matchers.expect("client routing matchers");
    assert!(matchers.contains("import { match as word }"));
    assert!(matchers.contains("export const matchers = { word };"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn generates_server_internal_from_project_state() {
    let cwd = temp_dir("server-internal");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
export default {
	compilerOptions: {
		experimental: {
			async: true
		}
	},
	kit: {
		csrf: {
			trustedOrigins: ['https://trusted.example']
		},
		serviceWorker: {
			options: {
				type: 'module'
			}
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("src").join("app.html"),
        "<html>%sveltekit.head%<body data-version=\"%sveltekit.version%\">%sveltekit.body% %sveltekit.env.PUBLIC_FOO% %sveltekit.nonce%</body></html>",
    );
    write_file(
        &cwd.join("src").join("error.html"),
        "<h1>%sveltekit.status%:%sveltekit.error.message%</h1>",
    );
    write_file(
        &cwd.join("src").join("hooks.server.ts"),
        "export function handle() {}\nexport function init() {}\n",
    );
    write_file(
        &cwd.join("src").join("hooks.ts"),
        "export function reroute() {}\nexport const transport = {};\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("service-worker.ts"),
        "export const version = '1';\n",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let generated = generate_server_internal(
        &project.config,
        &project.config.kit.out_dir.join("generated"),
        &runtime_directory,
        "root.js",
    )
    .expect("generate server internal");

    assert!(
        generated
            .contents
            .contains("import root from '../root.js';")
    );
    assert!(
        generated
            .contents
            .contains("set_private_env, set_public_env")
    );
    assert!(generated.contents.contains("async: true"));
    assert!(generated.contents.contains("csrf_check_origin: true"));
    assert!(
        generated
            .contents
            .contains("csrf_trusted_origins: [\"https://trusted.example\"]")
    );
    assert!(generated.contents.contains("service_worker: true"));
    assert!(
        generated
            .contents
            .contains("service_worker_options: {\"type\":\"module\"}")
    );
    assert!(generated.contents.contains("data-version=\\\""));
    assert!(generated.contents.contains("\" + head + \""));
    assert!(generated.contents.contains("\" + body + \""));
    assert!(
        generated
            .contents
            .contains("\" + (env[\"PUBLIC_FOO\"] ?? \"\") + \"")
    );
    assert!(generated.contents.contains("\" + nonce + \""));
    assert!(generated.contents.contains("\" + status + \""));
    assert!(generated.contents.contains("\" + message + \""));
    assert!(
        generated
            .contents
            .contains("await import(\"../../../src/hooks.server.ts\")")
    );
    assert!(
        generated
            .contents
            .contains("await import(\"../../../src/hooks.ts\")")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn generates_root_component_from_nested_layout_depth() {
    let cwd = temp_dir("root-component");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("dashboard")
            .join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("dashboard")
            .join("reports")
            .join("+page.svelte"),
        "<h1>reports</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_root(&project.manifest);

    assert!(generated.svelte.contains("<svelte:options runes={true} />"));
    assert!(
        generated
            .svelte
            .contains("const Pyramid_3 = $derived(constructors[3])")
    );
    assert!(
        generated
            .svelte
            .contains("data_0 = null, data_1 = null, data_2 = null, data_3 = null")
    );
    assert!(
        generated
            .svelte
            .contains("<Pyramid_0 bind:this={components[0]}")
    );
    assert!(
        generated
            .svelte
            .contains("<Pyramid_1 bind:this={components[1]}")
    );
    assert!(
        generated
            .svelte
            .contains("<Pyramid_2 bind:this={components[2]}")
    );
    assert!(
        generated
            .svelte
            .contains("<Pyramid_3 bind:this={components[3]}")
    );
    assert!(
        generated
            .js
            .contains("import { asClassComponent } from 'svelte/legacy';")
    );
    assert!(generated.js.contains("import Root from './root.svelte';"));
    assert!(
        generated
            .js
            .contains("export default asClassComponent(Root);")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn generates_non_ambient_types_from_manifest_state() {
    let cwd = temp_dir("non-ambient");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("(group)")
            .join("[slug]")
            .join("+page.svelte"),
        "<h1>slug</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("(group)")
            .join("[slug]")
            .join("+page.ts"),
        "export const trailingSlash = 'always';\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[[category]]")
            .join("+server.ts"),
        "export const trailingSlash = 'ignore';\n",
    );
    write_file(&cwd.join("static").join("robots.txt"), "User-agent: *\n");

    let project = load_project(&cwd).expect("load project");
    let route = project
        .manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/(group)/[slug]")
        .and_then(|route| route.page.as_ref())
        .expect("group slug page route");
    assert_eq!(
        RuntimePageNodes::from_route(route, &project.manifest).trailing_slash(),
        "always"
    );
    let generated = generate_non_ambient(&project.manifest);

    assert!(
        generated
            .contents
            .contains("// this file is generated — do not edit it")
    );
    assert!(
        generated
            .contents
            .contains("declare module \"svelte/elements\"")
    );
    assert!(
        generated
            .contents
            .contains("'data-sveltekit-preload-code'?")
    );
    assert!(generated.contents.contains("declare module \"$app/types\""));
    assert!(generated.contents.contains("RouteId():"));
    assert!(generated.contents.contains("\"/blog/[[category]]\""));
    assert!(generated.contents.contains("\"/(group)/[slug]\""));
    assert!(
        generated
            .contents
            .contains("\"/(group)/[slug]\": { slug: string }")
    );
    assert!(
        generated
            .contents
            .contains("\"/blog/[[category]]\": { category?: string }")
    );
    assert!(
        generated
            .contents
            .contains("\"/(group)/[slug]\": { slug: string }")
    );
    assert!(generated.contents.contains("`/${string}/` & {}"));
    assert!(generated.contents.contains("`/blog${string}` & {}"));
    assert!(
        generated
            .contents
            .contains("Asset(): \"/robots.txt\" | string & {}")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn generates_ambient_types_from_env_files() {
    let cwd = temp_dir("ambient");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
export default {
	kit: {
		env: {
			publicPrefix: 'PUBLIC_',
			privatePrefix: 'PRIVATE_'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join(".env"),
        "PUBLIC_FOO=public\nPRIVATE_BAR=private\nSHARED=ignored\nfor=reserved\n",
    );
    write_file(&cwd.join(".env.dev"), "PUBLIC_DEV=dev-mode\n");

    let project = load_project(&cwd).expect("load project");
    let generated = generate_ambient(&project.config, "dev");

    assert!(
        generated
            .contents
            .contains("// this file is generated — do not edit it")
    );
    assert!(
        generated
            .contents
            .contains("/// <reference types=\"@sveltejs/kit\" />")
    );
    assert!(
        generated
            .contents
            .contains("declare module '$env/static/private'")
    );
    assert!(
        generated
            .contents
            .contains("export const PRIVATE_BAR: string;")
    );
    assert!(!generated.contents.contains("export const for: string;"));
    assert!(
        generated
            .contents
            .contains("declare module '$env/static/public'")
    );
    assert!(
        generated
            .contents
            .contains("export const PUBLIC_FOO: string;")
    );
    assert!(
        generated
            .contents
            .contains("export const PUBLIC_DEV: string;")
    );
    assert!(
        generated
            .contents
            .contains("declare module '$env/dynamic/private'")
    );
    assert!(
        generated
            .contents
            .contains("[key: `PUBLIC_${string}`]: undefined;")
    );
    assert!(
        generated
            .contents
            .contains("[key: `PRIVATE_${string}`]: string | undefined;")
    );
    assert!(
        generated
            .contents
            .contains("declare module '$env/dynamic/public'")
    );
    assert!(
        generated
            .contents
            .contains("[key: `PRIVATE_${string}`]: undefined;")
    );
    assert!(
        generated
            .contents
            .contains("[key: `PUBLIC_${string}`]: string | undefined;")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn generates_tsconfig_from_project_state() {
    let cwd = temp_dir("tsconfig");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
export default {
	kit: {
		alias: {
			simpleKey: 'simple/value',
			key: 'value',
			'key/*': 'some/other/value/*',
			keyToFile: 'path/to/file.ts',
			$routes: '.svelte-kit/types/src/routes'
		},
		files: {
			lib: 'app',
			serviceWorker: 'src/sw'
		},
		typescript: {
			config: (config) => ({
				...config,
				extends: 'some/other/tsconfig.json'
			})
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("app").join("stores.ts"),
        "export const value = 1;\n",
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_tsconfig(&cwd, &project.config.kit);
    let json = serde_json::from_str::<Value>(&generated.contents).expect("generated tsconfig json");

    assert!(generated.warnings.is_empty());
    assert!(
        generated
            .custom_hook_source
            .as_deref()
            .is_some_and(|source| source.contains("extends"))
    );

    let paths = json["compilerOptions"]["paths"]
        .as_object()
        .expect("compilerOptions.paths object");
    assert_eq!(
        paths["$app/types"],
        serde_json::json!(["./types/index.d.ts"])
    );
    assert_eq!(paths["$lib"], serde_json::json!(["../app"]));
    assert_eq!(paths["$lib/*"], serde_json::json!(["../app/*"]));
    assert_eq!(paths["simpleKey"], serde_json::json!(["../simple/value"]));
    assert_eq!(
        paths["simpleKey/*"],
        serde_json::json!(["../simple/value/*"])
    );
    assert_eq!(paths["key"], serde_json::json!(["../value"]));
    assert_eq!(paths["key/*"], serde_json::json!(["../some/other/value/*"]));
    assert_eq!(
        paths["keyToFile"],
        serde_json::json!(["../path/to/file.ts"])
    );
    assert_eq!(paths["$routes"], serde_json::json!(["./types/src/routes"]));
    assert_eq!(
        paths["$routes/*"],
        serde_json::json!(["./types/src/routes/*"])
    );

    assert_eq!(
        json["compilerOptions"]["rootDirs"],
        serde_json::json!(["..", "./types"])
    );
    let include = json["include"].as_array().expect("include array");
    for entry in [
        "ambient.d.ts",
        "non-ambient.d.ts",
        "./types/**/$types.d.ts",
        "../vite.config.js",
        "../vite.config.ts",
        "../app/**/*.js",
        "../app/**/*.ts",
        "../app/**/*.svelte",
        "../src/**/*.js",
        "../src/**/*.ts",
        "../src/**/*.svelte",
        "../test/**/*.js",
        "../test/**/*.ts",
        "../test/**/*.svelte",
        "../tests/**/*.js",
        "../tests/**/*.ts",
        "../tests/**/*.svelte",
    ] {
        assert!(include.contains(&Value::String(entry.to_string())));
    }

    let exclude = json["exclude"].as_array().expect("exclude array");
    for entry in [
        "../node_modules/**",
        "../src/sw.js",
        "../src/sw/**/*.js",
        "../src/sw.ts",
        "../src/sw/**/*.ts",
        "../src/sw.d.ts",
        "../src/sw/**/*.d.ts",
    ] {
        assert!(exclude.contains(&Value::String(entry.to_string())));
    }

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn excludes_single_service_worker_entry_from_tsconfig() {
    let cwd = temp_dir("tsconfig-service-worker-file");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
export default {
	kit: {
		files: {
			serviceWorker: 'src/service-worker.ts'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_tsconfig(&cwd, &project.config.kit);
    let json = serde_json::from_str::<Value>(&generated.contents).expect("generated tsconfig json");

    assert!(generated.warnings.is_empty());
    assert_eq!(
        json["exclude"],
        serde_json::json!(["../node_modules/**", "../src/service-worker.ts"])
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn warns_when_user_tsconfig_does_not_extend_generated_config() {
    let cwd = temp_dir("tsconfig-warning-missing-extends");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("tsconfig.json"),
        r#"
{
  // comments are valid in tsconfig
  "compilerOptions": {
    "strict": true
  }
}
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_tsconfig(&cwd, &project.config.kit);

    assert_eq!(generated.warnings.len(), 1);
    assert!(
        generated.warnings[0]
            .contains("Your tsconfig.json should extend the configuration generated by SvelteKit")
    );
    assert!(generated.warnings[0].contains("\"extends\": \"./.svelte-kit/tsconfig.json\""));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn warns_when_user_tsconfig_overrides_paths_or_base_url() {
    let cwd = temp_dir("tsconfig-warning-paths");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("jsconfig.json"),
        r#"
{
  "extends": "./.svelte-kit/tsconfig.json",
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "$lib": ["src/lib"]
    }
  }
}
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let generated = generate_tsconfig(&cwd, &project.config.kit);

    assert_eq!(generated.warnings.len(), 1);
    assert!(
        generated.warnings[0]
            .contains("You have specified a baseUrl and/or paths in your jsconfig.json")
    );
    assert!(generated.warnings[0].contains("use `kit.alias` instead"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn writes_sync_artifacts_to_the_output_tree() {
    let cwd = temp_dir("write-sync");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
export default {
	kit: {
		alias: {
			$routes: '.svelte-kit/types/src/routes'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body% %sveltekit.env.PUBLIC_FOO%</body></html>",
    );
    write_file(
        &cwd.join("src").join("hooks.client.ts"),
        "export function init() {}\n",
    );
    write_file(
        &cwd.join("src").join("hooks.server.ts"),
        "export function handle() {}\n",
    );
    write_file(
        &cwd.join("src").join("params").join("word.ts"),
        "export function match(value) { return value.length > 0; }\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(&cwd.join(".env"), "PUBLIC_FOO=public\n");

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let first = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(first.tsconfig_warnings.is_empty());
    assert!(!first.written_files.is_empty());

    let out_dir = project.config.kit.out_dir.clone();
    assert!(out_dir.join("tsconfig.json").is_file());
    assert!(out_dir.join("ambient.d.ts").is_file());
    assert!(out_dir.join("non-ambient.d.ts").is_file());
    assert!(out_dir.join("generated").join("root.svelte").is_file());
    assert!(out_dir.join("generated").join("root.js").is_file());
    assert!(
        out_dir
            .join("generated")
            .join("client")
            .join("app.js")
            .is_file()
    );
    assert!(
        out_dir
            .join("generated")
            .join("client")
            .join("matchers.js")
            .is_file()
    );
    assert!(
        out_dir
            .join("generated")
            .join("client")
            .join("nodes")
            .join("0.js")
            .is_file()
    );
    assert!(
        out_dir
            .join("generated")
            .join("server")
            .join("internal.js")
            .is_file()
    );

    let tsconfig = fs::read_to_string(out_dir.join("tsconfig.json")).expect("read tsconfig");
    assert!(tsconfig.contains("\"$app/types\""));

    let ambient = fs::read_to_string(out_dir.join("ambient.d.ts")).expect("read ambient");
    assert!(ambient.contains("PUBLIC_FOO"));

    let app = fs::read_to_string(out_dir.join("generated").join("client").join("app.js"))
        .expect("read client app");
    assert!(app.contains("export { matchers } from './matchers.js';"));

    let second = write_sync_project(&project, "dev", &runtime_directory).expect("rewrite sync");
    assert!(second.written_files.is_empty());
    assert!(second.tsconfig_warnings.is_empty());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn initializes_sync_artifacts_without_generating_runtime_files() {
    let cwd = temp_dir("write-sync-init");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let result = init_sync_project(&project, "dev").expect("init sync");

    assert!(result.tsconfig_warnings.is_empty());
    assert!(project.config.kit.out_dir.join("tsconfig.json").is_file());
    assert!(project.config.kit.out_dir.join("ambient.d.ts").is_file());
    assert!(!project.config.kit.out_dir.join("non-ambient.d.ts").exists());
    assert!(!project.config.kit.out_dir.join("generated").exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn creates_sync_runtime_and_types_without_init_files() {
    let cwd = temp_dir("write-sync-create");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = create_sync_project(&project, &runtime_directory).expect("create sync");

    assert!(result.tsconfig_warnings.is_empty());
    assert!(!project.config.kit.out_dir.join("tsconfig.json").exists());
    assert!(!project.config.kit.out_dir.join("ambient.d.ts").exists());
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("non-ambient.d.ts")
            .is_file()
    );
    assert!(project.config.kit.out_dir.join("types").is_dir());
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("generated")
            .join("root.svelte")
            .is_file()
    );
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("generated")
            .join("client")
            .join("app.js")
            .is_file()
    );
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("generated")
            .join("server")
            .join("internal.js")
            .is_file()
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn writes_all_sync_types_without_runtime_files() {
    let cwd = temp_dir("write-sync-all-types");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let result = write_all_sync_types(&project, "dev").expect("write all sync types");

    assert!(result.tsconfig_warnings.is_empty());
    assert!(project.config.kit.out_dir.join("tsconfig.json").is_file());
    assert!(project.config.kit.out_dir.join("ambient.d.ts").is_file());
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("non-ambient.d.ts")
            .is_file()
    );
    assert!(project.config.kit.out_dir.join("types").is_dir());
    assert!(!project.config.kit.out_dir.join("generated").exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn writes_server_internal_without_other_sync_outputs() {
    let cwd = temp_dir("write-sync-server-only");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_server_project(&project, &runtime_directory).expect("write server");

    assert!(result.tsconfig_warnings.is_empty());
    assert!(
        project
            .config
            .kit
            .out_dir
            .join("generated")
            .join("server")
            .join("internal.js")
            .is_file()
    );
    assert!(
        !project
            .config
            .kit
            .out_dir
            .join("generated")
            .join("root.svelte")
            .exists()
    );
    assert!(!project.config.kit.out_dir.join("tsconfig.json").exists());
    assert!(!project.config.kit.out_dir.join("types").exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn updates_route_types_when_server_exports_change() {
    let cwd = temp_dir("write-sync-update-types");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    let server_file = cwd.join("src").join("routes").join("+page.server.ts");
    write_file(
        &server_file,
        "export const actions = { default: async () => ({ ok: true }) };\n",
    );

    let project = load_project(&cwd).expect("load project");
    let initial = create_sync_project(&project, &cwd.join("runtime")).expect("create sync");
    assert!(initial.tsconfig_warnings.is_empty());

    let types_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let initial_types = fs::read_to_string(&types_path).expect("read initial route types");
    assert!(initial_types.contains("export type SubmitFunction ="));

    write_file(
        &server_file,
        "export const load = async () => ({ ok: true as const });\n",
    );
    let update = update_sync_project_for_file(&project, &server_file).expect("update sync");
    assert!(update.tsconfig_warnings.is_empty());

    let updated_types = fs::read_to_string(types_path).expect("read updated route types");
    assert!(!updated_types.contains("export type SubmitFunction ="));
    assert!(updated_types.contains("export type ActionData = unknown;"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn updates_non_ambient_types_when_page_options_change() {
    let cwd = temp_dir("write-sync-update-non-ambient");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("+page.svelte"),
        "<h1>blog</h1>",
    );
    let page_file = cwd.join("src").join("routes").join("blog").join("+page.ts");
    write_file(&page_file, "export const trailingSlash = 'never';\n");

    let project = load_project(&cwd).expect("load project");
    let initial = write_all_sync_types(&project, "dev").expect("write sync types");
    assert!(initial.tsconfig_warnings.is_empty());

    let non_ambient_path = project.config.kit.out_dir.join("non-ambient.d.ts");
    let initial_non_ambient = fs::read_to_string(&non_ambient_path).expect("read non-ambient");
    assert!(initial_non_ambient.contains("\"/blog\""));
    assert!(!initial_non_ambient.contains("\"/blog/\""));

    write_file(&page_file, "export const trailingSlash = 'ignore';\n");
    let update = update_sync_project_for_file(&project, &page_file).expect("update sync");
    assert!(update.tsconfig_warnings.is_empty());

    let updated_non_ambient =
        fs::read_to_string(non_ambient_path).expect("read updated non-ambient");
    assert!(updated_non_ambient.contains("\"/blog\""));
    assert!(updated_non_ambient.contains("\"/blog/\""));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn removes_stale_route_type_files_when_routes_are_deleted() {
    let cwd = temp_dir("write-sync-remove-stale-route-types");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    let blog_page = cwd
        .join("src")
        .join("routes")
        .join("blog")
        .join("+page.svelte");
    write_file(&blog_page, "<h1>blog</h1>");

    let project = load_project(&cwd).expect("load project");
    write_all_types(&project).expect("write initial route types");

    let stale_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("blog")
        .join("$types.d.ts");
    assert!(stale_types.is_file());

    fs::remove_file(&blog_page).expect("remove blog page");
    let refreshed = load_project(&cwd).expect("reload project");
    write_all_types(&refreshed).expect("rewrite route types");

    assert!(!stale_types.exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn removes_stale_proxy_files_when_they_are_no_longer_needed() {
    let cwd = temp_dir("write-sync-remove-stale-proxy");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    let page_file = cwd.join("src").join("routes").join("+page.ts");
    write_file(
        &page_file,
        "import type { PageLoad } from './$types';\nexport const load: PageLoad = async () => ({ ok: true });\n",
    );

    let project = load_project(&cwd).expect("load project");
    write_all_types(&project).expect("write initial route types");

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.ts");
    assert!(proxy_path.is_file());

    write_file(
        &page_file,
        "export const load = async () => ({ ok: true });\n",
    );
    let refreshed = load_project(&cwd).expect("reload project");
    write_all_types(&refreshed).expect("rewrite route types");

    assert!(!proxy_path.exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn writes_route_types_for_pages_layouts_and_endpoints() {
    let cwd = temp_dir("write-route-types");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("params").join("word.ts"),
        "export function match(value: string): value is 'a' | 'b' { return value === 'a' || value === 'b'; }\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.server.ts"),
        "export const load = async () => ({ root: true });\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.ts"),
        "export const load = async () => ({ layout: 'root' as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.ts"),
        "export const load = async () => ({ slugged: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug=word]")
            .join("+page.server.ts"),
        "export const actions = { default: async () => ({ ok: true }) };\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("api")
            .join("[id]")
            .join("+server.ts"),
        "export function GET() {}\n",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let root_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let page_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("blog")
        .join("[slug=word]")
        .join("$types.d.ts");
    let endpoint_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("api")
        .join("[id]")
        .join("$types.d.ts");

    assert!(root_types.is_file());
    assert!(page_types.is_file());
    assert!(endpoint_types.is_file());

    let root = fs::read_to_string(root_types).expect("read root types");
    assert!(root.contains("type LayoutRouteId = \"/\" | \"/blog/[slug=word]\" | null;"));
    assert!(root.contains("export type LayoutServerLoad"));
    assert!(root.contains("type LayoutServerModule = typeof import("));
    assert!(root.contains("type LayoutModule = typeof import("));
    assert!(root.contains("type OutputDataShape<T> ="));
    assert!(root.contains("type OptionalUnion<U extends Record<string, any>"));
    assert!(root.contains(
        "export type LayoutServerData = Expand<EnsureDefined<ModuleLoadData<LayoutServerModule>>>;"
    ));
    assert!(root.contains("export type LayoutData = Expand<Omit<LayoutParentData, keyof ModuleLoadData<LayoutModule>> & OptionalUnion<EnsureDefined<ModuleLoadData<LayoutModule>>>>;"));
    assert!(root.contains("export type LayoutProps"));

    let page = fs::read_to_string(page_types).expect("read page types");
    assert!(page.contains("import { match as matcher_word }"));
    assert!(page.contains("type RouteParams = { slug: MatcherParam<typeof matcher_word> };"));
    assert!(page.contains("type PageModule = typeof import("));
    assert!(
        page.contains(
            "export type PageServerParentData = EnsureDefined<import(\"../../$types\").LayoutServerData>;"
        )
    );
    assert!(page.contains(
        "export type PageParentData = EnsureDefined<import(\"../../$types\").LayoutData>;"
    ));
    assert!(page.contains("export type PageServerData = null;"));
    assert!(page.contains("export type PageData = Expand<Omit<PageParentData, keyof ModuleLoadData<PageModule>> & OptionalUnion<EnsureDefined<ModuleLoadData<PageModule>>>>;"));
    assert!(page.contains("export type PageServerLoad"));
    assert!(page.contains("export type Action"));
    assert!(page.contains("export type Actions"));
    assert!(page.contains("type ActionsExport ="));
    assert!(page.contains("type ExcludeActionFailure<T> ="));
    assert!(
        page.contains("type ActionsSuccess<T extends Record<string, (...args: any) => any>> =")
    );
    assert!(page.contains("export type ActionData = ActionsExport extends Record<string, (...args: any) => any> ? Expand<Kit.AwaitedActions<ActionsExport>> | null : unknown;"));
    assert!(page.contains("export type SubmitFunction = ActionsExport extends Record<string, (...args: any) => any> ? Kit.SubmitFunction<Expand<ActionsSuccess<ActionsExport>>, Expand<ActionsFailure<ActionsExport>>> : never;"));
    assert!(page.contains(
        "export type PageProps = { params: RouteParams; data: PageData; form: ActionData };"
    ));

    let endpoint = fs::read_to_string(endpoint_types).expect("read endpoint types");
    assert!(endpoint.contains("type RouteParams = { id: string };"));
    assert!(
        endpoint.contains("export type RequestHandler = Kit.RequestHandler<RouteParams, RouteId>;")
    );
    assert!(
        endpoint.contains("export type RequestEvent = Kit.RequestEvent<RouteParams, RouteId>;")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn accumulates_parent_layout_data_across_nested_route_types() {
    let cwd = temp_dir("write-route-types-nested-parents");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.server.ts"),
        "export const load = async () => ({ root_server: true as const });\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.ts"),
        "export const load = async () => ({ root_layout: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("+layout.server.ts"),
        "export const load = async () => ({ blog_server: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("+layout.ts"),
        "export const load = async () => ({ blog_layout: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.server.ts"),
        "export const load = async () => ({ page_server: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.ts"),
        "export const load = async () => ({ page_layout: true as const });\n",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let blog_layout_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("blog")
        .join("$types.d.ts");
    let page_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("blog")
        .join("[slug]")
        .join("$types.d.ts");

    let blog_layout = fs::read_to_string(blog_layout_types).expect("read blog layout types");
    assert!(blog_layout.contains(
        "type LayoutServerParentData = EnsureDefined<import(\"../$types\").LayoutServerData>;"
    ));
    assert!(
        blog_layout
            .contains("type LayoutParentData = EnsureDefined<import(\"../$types\").LayoutData>;")
    );

    let page = fs::read_to_string(page_types).expect("read page types");
    assert!(page.contains(
        "type PageServerParentData = Omit<EnsureDefined<import(\"../../$types\").LayoutServerData>, keyof import(\"../$types\").LayoutServerData> & EnsureDefined<import(\"../$types\").LayoutServerData>;"
    ));
    assert!(page.contains(
        "type PageParentData = Omit<EnsureDefined<import(\"../../$types\").LayoutData>, keyof import(\"../$types\").LayoutData> & EnsureDefined<import(\"../$types\").LayoutData>;"
    ));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn omits_submit_function_when_page_server_has_no_actions() {
    let cwd = temp_dir("write-route-types-no-actions");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.ts"),
        "export const load = async () => ({ server: true as const });\n",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let types_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let types = fs::read_to_string(types_path).expect("read route types");

    assert!(types.contains("export type ActionData = unknown;"));
    assert!(!types.contains("export type SubmitFunction ="));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn uses_partial_page_data_output_shape_for_layouts_when_all_child_pages_have_load() {
    let cwd = temp_dir("write-route-types-layout-output-shape");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.server.ts"),
        "export const load = async () => ({ layout_server: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("a")
            .join("+page.svelte"),
        "<h1>a</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("a").join("+page.ts"),
        "export const load = async () => ({ a: true as const });\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("b")
            .join("+page.svelte"),
        "<h1>b</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("b")
            .join("+page.server.ts"),
        "export const load = async () => ({ b: true as const });\n",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let root_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let root = fs::read_to_string(root_types).expect("read root types");

    assert!(root.contains(
        "export type LayoutServerLoad<OutputData extends Partial<App.PageData> & Record<string, any> | void = Partial<App.PageData> & Record<string, any> | void>"
    ));
    assert!(root.contains(
        "export type LayoutLoad<OutputData extends Partial<App.PageData> & Record<string, any> | void = Partial<App.PageData> & Record<string, any> | void>"
    ));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn excludes_named_layout_escape_pages_from_nested_layout_params() {
    let cwd = temp_dir("write-route-types-layout-params-named-escape");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("nested")
            .join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("nested")
            .join("[...rest]")
            .join("+page.svelte"),
        "<h1>rest</h1>",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("nested")
            .join("[slug]")
            .join("+page@.svelte"),
        "<h1>slug</h1>",
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let nested_types = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("nested")
        .join("$types.d.ts");
    let nested = fs::read_to_string(nested_types).expect("read nested layout types");

    assert!(nested.contains("type LayoutParams = RouteParams & { rest?: string };"));
    assert!(!nested.contains("slug?: string"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn sync_smoke_tests_upstream_write_types_fixtures() {
    let fixture_root = repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("sync")
        .join("write_types")
        .join("test");

    for entry in fs::read_dir(&fixture_root).expect("read fixture root") {
        let entry = entry.expect("fixture entry");
        if !entry.file_type().expect("fixture file type").is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let project = build_fixture_project(&name);
        let first = write_all_types(&project).expect("write fixture types");
        assert!(
            !first.is_empty(),
            "expected generated files for fixture {name}"
        );
        assert!(
            project.config.kit.out_dir.join("types").is_dir(),
            "missing types output for fixture {name}"
        );

        let second = write_all_types(&project).expect("rewrite fixture types");
        assert!(
            second.is_empty(),
            "fixture {name} should be stable on second write"
        );

        fs::remove_dir_all(&project.cwd).expect("remove fixture temp dir");
    }
}

#[test]
fn typechecks_actions_write_types_fixture_with_tsc() {
    let project = build_workspace_fixture_project("actions");
    write_fixture_type_artifacts(&project);
    stabilize_workspace_fixture_typecheck(&project, "actions");

    let output = Command::new("cmd")
        .arg("/C")
        .arg("tsc")
        .arg("-p")
        .arg("tsconfig.json")
        .current_dir(project.config.kit.files.routes.as_std_path())
        .output()
        .expect("run actions fixture tsc");

    assert!(
        output.status.success(),
        "actions fixture typecheck failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(
        project
            .cwd
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.parent())
            .expect("workspace temp root"),
    )
    .expect("remove workspace fixture temp dir");
}

#[test]
fn typechecks_app_types_write_types_fixture_with_tsc() {
    let project = build_workspace_fixture_project("app-types");
    write_fixture_type_artifacts(&project);
    stabilize_workspace_fixture_typecheck(&project, "app-types");

    let output = Command::new("cmd")
        .arg("/C")
        .arg("tsc")
        .arg("-p")
        .arg("tsconfig.json")
        .current_dir(project.config.kit.files.routes.as_std_path())
        .output()
        .expect("run app-types fixture tsc");

    assert!(
        output.status.success(),
        "app-types fixture typecheck failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(
        project
            .cwd
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.parent())
            .expect("workspace temp root"),
    )
    .expect("remove workspace fixture temp dir");
}

#[test]
fn typechecks_layout_write_types_fixture_with_tsc() {
    let project = build_workspace_fixture_project("layout");
    write_fixture_type_artifacts(&project);
    stabilize_workspace_fixture_typecheck(&project, "layout");

    let output = Command::new("cmd")
        .arg("/C")
        .arg("tsc")
        .arg("-p")
        .arg("tsconfig.json")
        .current_dir(project.config.kit.files.routes.as_std_path())
        .output()
        .expect("run layout fixture tsc");

    assert!(
        output.status.success(),
        "layout fixture typecheck failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(
        project
            .cwd
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.parent())
            .expect("workspace temp root"),
    )
    .expect("remove workspace fixture temp dir");
}

#[test]
fn typechecks_upstream_write_types_fixtures_with_tsc() {
    let fixture_root = repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("sync")
        .join("write_types")
        .join("test");

    for entry in fs::read_dir(&fixture_root).expect("read fixture root") {
        let entry = entry.expect("fixture entry");
        if !entry.file_type().expect("fixture file type").is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            "actions" | "app-types" | "layout" | "layout-advanced" | "simple-page-server-only"
        ) {
            continue;
        }
        let project = build_workspace_fixture_project(&name);
        write_fixture_type_artifacts(&project);
        stabilize_workspace_fixture_typecheck(&project, &name);

        let output = Command::new("cmd")
            .arg("/C")
            .arg("tsc")
            .arg("-p")
            .arg("tsconfig.json")
            .current_dir(project.config.kit.files.routes.as_std_path())
            .output()
            .expect("run fixture tsc");

        assert!(
            output.status.success(),
            "fixture {name} typecheck failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        fs::remove_dir_all(
            project
                .cwd
                .parent()
                .and_then(|parent| parent.parent())
                .and_then(|parent| parent.parent())
                .expect("workspace temp root"),
        )
        .expect("remove workspace fixture temp dir");
    }
}

#[test]
fn writes_proxy_modules_for_types_annotated_route_modules() {
    let cwd = temp_dir("write-route-proxy-types");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.ts"),
        r#"
import type { PageLoad } from './$types';

export const load: PageLoad = async () => {
	return {
		home: true as const
	};
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let types_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.ts");

    assert!(proxy_path.is_file());

    let types = fs::read_to_string(types_path).expect("read route types");
    assert!(types.contains("type PageModule = typeof import(\"./proxy+page.js\");"));

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("import type { PageLoad } from './$types';"));
    assert!(proxy.contains("export const load = async () => {"));
    assert!(proxy.contains(";null as any as PageLoad;"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_jsdoc_types_referencing_route_types_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-jsdoc");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.js"),
        r#"
/** @type {import('./$types').PageLoad} */
export function load({ params }) {
	return {
		home: params
	};
}
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let types_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");
    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.js");

    assert!(proxy_path.is_file());

    let types = fs::read_to_string(types_path).expect("read route types");
    assert!(types.contains("type PageModule = typeof import(\"./proxy+page.js\");"));

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("/** @type {import('./$types').PageLoad} */"));
    assert!(proxy.contains("export function load({ params }) {"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_jsdoc_function_type_tags_to_param_tags_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-jsdoc-function");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.js"),
        r#"
/** @type {import('./$types').PageLoad} */
export function load(event) {
	return {
		home: event.params
	};
}
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.js");

    assert!(proxy_path.is_file());

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("@type {import('./$types').PageLoad}"));
    assert!(proxy.contains("/** @param {Parameters<import('./$types').PageLoad>[0]} event */"));
    assert!(proxy.contains("export function load(event) {"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_jsdoc_const_load_type_tags_to_param_tags_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-jsdoc-const");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.js"),
        r#"
/** @type {import('./$types').PageLoad} */
export const load = ({ params }) => {
	return {
		home: params
	};
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.js");

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("@type {import('./$types').PageLoad}"));
    assert!(proxy.contains("/** @param {Parameters<import('./$types').PageLoad>[0]} event */"));
    assert!(proxy.contains("export const load = ({ params }) => {"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_layout_jsdoc_load_type_tags_to_layout_param_tags_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-jsdoc-layout");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("[slug]")
            .join("+page.svelte"),
        "<h1>slug</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.js"),
        r#"
/** @type {import('./$types').LayoutLoad} */
export function load(event) {
	return {
		home: event.params
	};
}
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+layout.js");

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("@type {import('./$types').LayoutLoad}"));
    assert!(proxy.contains("/** @param {Parameters<import('./$types').LayoutLoad>[0]} event */"));
    assert!(!proxy.contains("PageLoad"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn appends_ts_nocheck_after_ts_check_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-ts-check");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.js"),
        r#"// @ts-check
/** @type {import('./$types').PageLoad} */
export const load = ({ params }) => {
	return {
		home: params
	};
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.js");

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.starts_with("// @ts-check\n// @ts-nocheck\n"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_typed_actions_object_entries_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-actions");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.ts"),
        r#"
import type { Actions } from './$types';

export const actions: Actions = {
	default: async (event) => {
		return {
			ok: event.params
		};
	}
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.server.ts");

    assert!(proxy_path.is_file());

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(!proxy.contains("import type { Actions } from './$types';"));
    assert!(proxy.contains("export const actions = {"));
    assert!(proxy.contains("default: async (event: import('./$types').RequestEvent) => {"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn preserves_remaining_types_imports_in_action_proxies() {
    let cwd = temp_dir("write-route-proxy-actions-imports");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.ts"),
        r#"
import type { Actions, RequestEvent } from './$types';

export const actions: Actions = {
	typed: async (event: RequestEvent) => {
		return {
			typed: event.params
		};
	},
	untyped: async (event) => {
		return {
			untyped: event.params
		};
	}
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.server.ts");

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("import type { RequestEvent } from './$types';"));
    assert!(!proxy.contains("import type { Actions, RequestEvent } from './$types';"));
    assert!(proxy.contains("typed: async (event: RequestEvent) => {"));
    assert!(proxy.contains("untyped: async (event: import('./$types').RequestEvent) => {"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn leaves_satisfies_based_route_modules_unproxied() {
    let cwd = temp_dir("write-route-proxy-satisfies");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.ts"),
        r#"
import type { Actions, PageServerLoad, RequestEvent } from './$types';

export function load({ params }) {
	return {
		home: params
	};
} satisfies PageServerLoad;

export const actions = {
	typed: async (event: RequestEvent) => {
		return {
			typed: event.params
		};
	},
	untyped: async (event) => {
		return {
			untyped: event.params
		};
	}
} satisfies Actions;
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.server.ts");
    let types_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("$types.d.ts");

    assert!(!proxy_path.exists());

    let types = fs::read_to_string(types_path).expect("read route types");
    assert!(!types.contains("proxy+page.server"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rewrites_jsdoc_actions_object_and_action_entries_in_proxy_modules() {
    let cwd = temp_dir("write-route-proxy-actions-jsdoc");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.js"),
        r#"
/** @type {import('./$types').Actions} */
export const actions = {
	a: () => {},
	b: (param) => {},
	/** @type {import('./$types').Action} */
	c: (param) => {},
};
"#,
    );

    let project = load_project(&cwd).expect("load project");
    let runtime_directory = cwd.join("runtime");
    let result = write_sync_project(&project, "dev", &runtime_directory).expect("write sync");

    assert!(result.tsconfig_warnings.is_empty());

    let proxy_path = project
        .config
        .kit
        .out_dir
        .join("types")
        .join("src")
        .join("routes")
        .join("proxy+page.server.js");

    let proxy = fs::read_to_string(proxy_path).expect("read proxy module");
    assert!(proxy.contains("// @ts-nocheck"));
    assert!(proxy.contains("/** */"));
    assert!(
        proxy.contains("b: /** @param {import('./$types').RequestEvent} param */ (param) => {}")
    );
    assert!(proxy.contains("/** @param {Parameters<import('./$types').Action>[0]} param */"));
    assert!(proxy.contains("c: (param) => {}"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
