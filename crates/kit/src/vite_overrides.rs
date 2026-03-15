use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnforcedViteConfigNode {
    Leaf,
    Branch(BTreeMap<&'static str, EnforcedViteConfigNode>),
}

pub fn find_overridden_vite_config(config: &Value, resolved_config: &Value) -> Vec<String> {
    let mut overridden = Vec::new();
    find_overridden_config(
        config,
        resolved_config,
        &enforced_vite_config(),
        "",
        &mut overridden,
    );
    overridden
}

pub fn warn_overridden_vite_config(config: &Value, resolved_config: &Value) -> Option<String> {
    let overridden = find_overridden_vite_config(config, resolved_config);
    if overridden.is_empty() {
        None
    } else {
        Some(format!(
            "The following Vite config options will be overridden by SvelteKit:{}",
            overridden
                .into_iter()
                .map(|key| format!("\n  - {key}"))
                .collect::<String>()
        ))
    }
}

fn find_overridden_config(
    config: &Value,
    resolved_config: &Value,
    enforced: &EnforcedViteConfigNode,
    path: &str,
    out: &mut Vec<String>,
) {
    let (Some(config), Some(resolved_config), EnforcedViteConfigNode::Branch(children)) =
        (config.as_object(), resolved_config.as_object(), enforced)
    else {
        return;
    };

    for (key, enforced) in children {
        let (Some(config_value), Some(resolved_value)) =
            (config.get(*key), resolved_config.get(*key))
        else {
            continue;
        };

        match enforced {
            EnforcedViteConfigNode::Leaf => {
                if config_value != resolved_value {
                    out.push(format!("{path}{key}"));
                }
            }
            EnforcedViteConfigNode::Branch(_) => find_overridden_config(
                config_value,
                resolved_value,
                enforced,
                &format!("{path}{key}."),
                out,
            ),
        }
    }
}

fn enforced_vite_config() -> EnforcedViteConfigNode {
    use EnforcedViteConfigNode::{Branch, Leaf};

    Branch(BTreeMap::from([
        ("appType", Leaf),
        (
            "build",
            Branch(BTreeMap::from([
                ("cssCodeSplit", Leaf),
                ("emptyOutDir", Leaf),
                (
                    "lib",
                    Branch(BTreeMap::from([
                        ("entry", Leaf),
                        ("name", Leaf),
                        ("formats", Leaf),
                    ])),
                ),
                ("manifest", Leaf),
                ("outDir", Leaf),
                (
                    "rollupOptions",
                    Branch(BTreeMap::from([
                        ("input", Leaf),
                        (
                            "output",
                            Branch(BTreeMap::from([
                                ("format", Leaf),
                                ("entryFileNames", Leaf),
                                ("chunkFileNames", Leaf),
                                ("assetFileNames", Leaf),
                            ])),
                        ),
                        ("preserveEntrySignatures", Leaf),
                    ])),
                ),
                ("ssr", Leaf),
            ])),
        ),
        ("publicDir", Leaf),
        (
            "resolve",
            Branch(BTreeMap::from([(
                "alias",
                Branch(BTreeMap::from([
                    ("$app", Leaf),
                    ("$lib", Leaf),
                    ("$service-worker", Leaf),
                ])),
            )])),
        ),
        ("root", Leaf),
    ]))
}
