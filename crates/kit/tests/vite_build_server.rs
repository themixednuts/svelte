use std::collections::BTreeMap;
use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use svelte_kit::{
    BuildManifestChunk, BundleStrategy, ClientBuildAsset, KitManifest, ManifestNode,
    ManifestNodeKind, ServerNodeBuildInput, build_server_node_artifacts, build_server_nodes_plan,
    validate_config,
};
use svelte_kit::{
    InlineStylesExport, ServerNodeModulePlan, render_inline_stylesheet_module,
    render_server_node_module,
};

#[test]
fn renders_inline_stylesheet_module_contents() {
    let module = render_inline_stylesheet_module("entry.css", "\"body{color:red}\"");
    assert!(module.contains("// entry.css"));
    assert!(module.contains("export default \"body{color:red}\";"));
}

#[test]
fn renders_server_node_module_with_universal_and_server_exports() {
    let plan = ServerNodeModulePlan {
        index: 3,
        component_import: Some("../nodes/3.js".to_string()),
        universal_import: Some("../entries/page.js".to_string()),
        universal_id: Some("src/routes/+page.ts".to_string()),
        server_import: Some("../entries/page.server.js".to_string()),
        server_id: Some("src/routes/+page.server.ts".to_string()),
        imports: vec!["entry.js".to_string()],
        stylesheets: vec!["entry.css".to_string()],
        fonts: vec!["font.woff2".to_string()],
        inline_styles: BTreeMap::from([(
            "entry.css".to_string(),
            InlineStylesExport::Identifier("stylesheet_0".to_string()),
        )]),
    };

    let module = render_server_node_module(&plan);
    assert!(module.contains("import stylesheet_0 from '../stylesheets/entry.css.js';"));
    assert!(module.contains("export const index = 3;"));
    assert!(module.contains("export const universal_id = \"src/routes/+page.ts\";"));
    assert!(module.contains("export const server_id = \"src/routes/+page.server.ts\";"));
    assert!(module.contains("export const imports = [\"entry.js\"];"));
    assert!(module.contains("export const stylesheets = [\"entry.css\"];"));
    assert!(module.contains("export const fonts = [\"font.woff2\"];"));
    assert!(module.contains("\"entry.css\": stylesheet_0"));
}

fn chunk(
    file: &str,
    imports: &[&str],
    dynamic_imports: &[&str],
    css: &[&str],
    assets: &[&str],
) -> BuildManifestChunk {
    BuildManifestChunk {
        file: file.to_string(),
        imports: imports.iter().map(|value| (*value).to_string()).collect(),
        dynamic_imports: dynamic_imports
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        css: css.iter().map(|value| (*value).to_string()).collect(),
        assets: assets.iter().map(|value| (*value).to_string()).collect(),
    }
}

#[test]
fn builds_server_node_artifacts_with_eager_client_assets_and_inlined_css() {
    let validated = validate_config(
        &serde_json::json!({
            "kit": {
                "inlineStyleThreshold": 64,
                "output": { "bundleStrategy": "split" },
                "paths": { "relative": true }
            }
        }),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config");
    assert_eq!(validated.kit.output.bundle_strategy, BundleStrategy::Split);

    let node = ManifestNode {
        kind: ManifestNodeKind::Page,
        component: Some(Utf8PathBuf::from("src/routes/+page.svelte")),
        universal: Some(Utf8PathBuf::from("src/routes/+page.ts")),
        server: Some(Utf8PathBuf::from("src/routes/+page.server.ts")),
        parent_id: None,
        universal_page_options: None,
        server_page_options: None,
        page_options: None,
    };

    let server_manifest = BTreeMap::from([
        (
            "src/routes/+page.svelte".to_string(),
            chunk(
                "entries/pages/page.ssr.js",
                &[],
                &[],
                &["entry.css", "shared.css"],
                &["_app/immutable/assets/font.woff2"],
            ),
        ),
        (
            "src/routes/+page.ts".to_string(),
            chunk("entries/pages/page.universal.js", &[], &[], &[], &[]),
        ),
        (
            "src/routes/+page.server.ts".to_string(),
            chunk("entries/pages/page.server.js", &[], &[], &[], &[]),
        ),
    ]);

    let client_manifest = BTreeMap::from([
        (
            ".svelte-kit/generated/client-optimized/nodes/0.js".to_string(),
            chunk(
                "generated/nodes/0.js",
                &["shared.js"],
                &["lazy.js"],
                &["entry.css"],
                &["_app/immutable/assets/font.woff2"],
            ),
        ),
        (
            "shared.js".to_string(),
            chunk("chunks/shared.js", &[], &[], &["shared.css"], &[]),
        ),
        (
            "lazy.js".to_string(),
            chunk(
                "chunks/lazy.js",
                &[],
                &[],
                &["lazy.css"],
                &["_app/immutable/assets/logo.svg"],
            ),
        ),
    ]);

    let client_chunks = vec![
        ClientBuildAsset {
            file_name: "entry.css".to_string(),
            source: "body{background:url('../../logo.svg')}".to_string(),
        },
        ClientBuildAsset {
            file_name: "shared.css".to_string(),
            source: "body { color: red; padding: 1rem; margin: 0; }".repeat(4),
        },
    ];

    let artifacts = build_server_node_artifacts(&ServerNodeBuildInput {
        index: 0,
        node: &node,
        kit: &validated.kit,
        server_manifest: &server_manifest,
        client_manifest: Some(&client_manifest),
        client_chunks: Some(&client_chunks),
        client_entry_path: Some(".svelte-kit/generated/client-optimized/nodes/0.js"),
        assets_path: Some("/_app/immutable/assets"),
        static_assets: &BTreeSet::from(["logo.svg".to_string()]),
    })
    .expect("artifacts");

    assert_eq!(
        artifacts.module.component_import.as_deref(),
        Some("../entries/pages/page.ssr.js")
    );
    assert_eq!(
        artifacts.module.universal_import.as_deref(),
        Some("../entries/pages/page.universal.js")
    );
    assert_eq!(
        artifacts.module.server_import.as_deref(),
        Some("../entries/pages/page.server.js")
    );
    assert_eq!(
        artifacts.module.imports,
        vec!["chunks/shared.js", "generated/nodes/0.js"]
    );
    assert_eq!(
        artifacts.module.stylesheets,
        vec!["entry.css", "shared.css"]
    );
    assert_eq!(
        artifacts.module.fonts,
        vec!["_app/immutable/assets/font.woff2"]
    );
    assert_eq!(
        artifacts.module.inline_styles.get("entry.css"),
        Some(&InlineStylesExport::Identifier("stylesheet_0".to_string()))
    );
    assert_eq!(artifacts.inline_stylesheets.len(), 1);
    assert_eq!(artifacts.inline_stylesheets[0].output_file, "entry.css.js");
    assert!(
        artifacts.inline_stylesheets[0]
            .contents
            .contains("export default")
    );
}

#[test]
fn builds_all_server_node_output_files() {
    let validated = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "inlineStyleThreshold": 64,
                "output": { "bundleStrategy": "split" }
            }
        }),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config");

    let manifest = KitManifest {
        assets: vec![svelte_kit::Asset {
            file: Utf8PathBuf::from("logo.svg"),
            size: 4,
            type_: Some("image/svg+xml".to_string()),
        }],
        hooks: svelte_kit::Hooks::default(),
        matchers: BTreeMap::new(),
        manifest_routes: Vec::new(),
        nodes: vec![ManifestNode {
            kind: ManifestNodeKind::Page,
            component: Some(Utf8PathBuf::from("src/routes/+page.svelte")),
            universal: Some(Utf8PathBuf::from("src/routes/+page.ts")),
            server: None,
            parent_id: None,
            universal_page_options: None,
            server_page_options: None,
            page_options: None,
        }],
        routes: Vec::new(),
    };
    let server_manifest = BTreeMap::from([
        (
            "src/routes/+page.svelte".to_string(),
            chunk("entries/page.ssr.js", &[], &[], &["entry.css"], &[]),
        ),
        (
            "src/routes/+page.ts".to_string(),
            chunk("entries/page.universal.js", &[], &[], &[], &[]),
        ),
    ]);
    let client_manifest = BTreeMap::from([(
        ".svelte-kit/generated/client-optimized/nodes/0.js".to_string(),
        chunk("generated/nodes/0.js", &[], &[], &["entry.css"], &[]),
    )]);
    let client_chunks = vec![ClientBuildAsset {
        file_name: "entry.css".to_string(),
        source: "body{color:red}".to_string(),
    }];

    let plan = build_server_nodes_plan(
        "E:/Projects/svelte/.svelte-kit/output",
        &validated.kit,
        &manifest,
        &server_manifest,
        Some(".svelte-kit/generated/client-optimized/nodes"),
        Some(&client_manifest),
        Some("/_app/immutable/assets"),
        Some(&client_chunks),
    )
    .expect("plan");

    assert_eq!(plan.node_modules.len(), 1);
    assert_eq!(
        plan.node_modules[0].output_path,
        "E:/Projects/svelte/.svelte-kit/output/server/nodes/0.js"
    );
    assert!(
        plan.node_modules[0]
            .contents
            .contains("export const index = 0;")
    );
    assert_eq!(plan.stylesheet_modules.len(), 1);
    assert_eq!(
        plan.stylesheet_modules[0].output_path,
        "E:/Projects/svelte/.svelte-kit/output/server/stylesheets/entry.css.js"
    );
}
