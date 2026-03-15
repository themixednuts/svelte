use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    BuilderPrerenderOption, BuilderRouteApi, BuilderRoutePage, BuilderServerMetadata,
    BuilderServerMetadataRoute, FallbackGenerationPlan, build_fallback_generation_plan,
    build_prerender_execution_plan, validate_config,
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
        .join(format!("svelte-kit-postbuild-prerender-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn builds_fallback_generation_plan() {
    let cwd = temp_dir("fallback");
    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "paths": {
                    "base": "/base"
                },
                "prerender": {
                    "origin": "https://example.com"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let env = BTreeMap::from([("PUBLIC_FOO".to_string(), "bar".to_string())]);
    let plan = build_fallback_generation_plan(
        &cwd,
        &config.kit,
        ".svelte-kit/output/server/manifest.js",
        &env,
    );

    assert_eq!(
        plan,
        FallbackGenerationPlan {
            output_root: Utf8PathBuf::from(".svelte-kit/output"),
            manifest_path: Utf8PathBuf::from(".svelte-kit/output/server/manifest.js"),
            server_internal_path: Utf8PathBuf::from(".svelte-kit/output/server/internal.js"),
            server_index_path: Utf8PathBuf::from(".svelte-kit/output/server/index.js"),
            assets_dir: Utf8PathBuf::from("static"),
            request_path: "/[fallback]".to_string(),
            request_url: "https://example.com/[fallback]".to_string(),
            env,
        }
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn builds_prerender_execution_plan_and_collects_static_files() {
    let cwd = temp_dir("prerender");
    fs::create_dir_all(cwd.join(".svelte-kit/output/client/_app/immutable"))
        .expect("create immutable dir");
    fs::create_dir_all(cwd.join(".svelte-kit/output/server/_app/immutable"))
        .expect("create server immutable dir");
    fs::write(cwd.join(".svelte-kit/output/client/entry.js"), "").expect("write entry");
    fs::write(
        cwd.join(".svelte-kit/output/server/_app/immutable/chunk.js"),
        "",
    )
    .expect("write immutable chunk");

    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "appDir": "_app",
                "outDir": ".svelte-kit",
                "paths": {
                    "base": "/base"
                },
                "prerender": {
                    "origin": "https://example.com",
                    "entries": ["/", "/docs"],
                    "crawl": true,
                    "concurrency": 3
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([
            (
                "/".to_string(),
                BuilderServerMetadataRoute {
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    api: BuilderRouteApi::default(),
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::True),
                    entries: None,
                    config: serde_json::Value::Null,
                },
            ),
            (
                "/blog".to_string(),
                BuilderServerMetadataRoute {
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    api: BuilderRouteApi::default(),
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::Auto),
                    entries: Some(vec!["/blog/hello".to_string()]),
                    config: serde_json::Value::Null,
                },
            ),
        ]),
    };

    let env = BTreeMap::from([("PUBLIC_FOO".to_string(), "bar".to_string())]);
    let plan = build_prerender_execution_plan(
        &cwd,
        &config.kit,
        ".svelte-kit/output/server/manifest.js",
        &metadata,
        false,
        &env,
    )
    .expect("plan should build");

    assert_eq!(
        plan.manifest_path,
        Utf8PathBuf::from(".svelte-kit/output/server/manifest.js")
    );
    assert_eq!(
        plan.server_internal_path,
        Utf8PathBuf::from(".svelte-kit/output/server/internal.js")
    );
    assert_eq!(
        plan.server_index_path,
        Utf8PathBuf::from(".svelte-kit/output/server/index.js")
    );
    assert_eq!(plan.origin, "https://example.com");
    assert_eq!(plan.entries, vec!["/".to_string(), "/docs".to_string()]);
    assert_eq!(plan.concurrency, 3);
    assert!(plan.crawl);
    assert_eq!(plan.remote_prefix, "/base/_app/remote/");
    assert_eq!(plan.static_files, vec!["_app/env.js", "entry.js"]);
    assert_eq!(plan.server_immutable_files, vec!["_app/immutable/chunk.js"]);
    assert_eq!(
        plan.prerender_map.get("/"),
        Some(&BuilderPrerenderOption::True)
    );
    assert_eq!(
        plan.prerender_map.get("/blog"),
        Some(&BuilderPrerenderOption::Auto)
    );
    assert!(plan.fallback.is_none());

    let hash_plan = build_prerender_execution_plan(
        &cwd,
        &config.kit,
        ".svelte-kit/output/server/manifest.js",
        &metadata,
        true,
        &BTreeMap::new(),
    )
    .expect("hash plan should build");
    assert!(hash_plan.hash);
    assert!(hash_plan.fallback.is_some());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
