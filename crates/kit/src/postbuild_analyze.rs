use std::collections::BTreeMap;

use camino::Utf8Path;

use crate::{
    AdapterFeatures, AnalyzedMetadata, AnalyzedRemoteExport, BuildData, BuildServerNodesPlan,
    RemoteExport, Result, ValidatedConfig, analyze_remote_metadata,
    analyze_server_metadata_with_features, build_server_nodes_plan,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostbuildAnalyzeResult {
    pub metadata: AnalyzedMetadata,
    pub remotes: BTreeMap<String, BTreeMap<String, AnalyzedRemoteExport>>,
    pub server_nodes_plan: BuildServerNodesPlan,
}

pub fn analyze_postbuild(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    build_data: &BuildData<'_>,
    tracked_features: Option<&BTreeMap<String, Vec<String>>>,
    adapter: Option<&AdapterFeatures>,
    remotes: &BTreeMap<String, BTreeMap<String, RemoteExport>>,
    generated_client_nodes_dir: Option<&str>,
) -> Result<PostbuildAnalyzeResult> {
    let metadata = analyze_server_metadata_with_features(
        cwd,
        config,
        build_data.manifest_data,
        Some(build_data),
        tracked_features,
        adapter,
    )?;
    let remotes = analyze_remote_metadata(remotes)?;
    let server_nodes_plan = build_server_nodes_plan(
        build_data.out_dir.as_str(),
        &config.kit,
        build_data.manifest_data,
        build_data.server_manifest,
        generated_client_nodes_dir,
        None,
        None,
        None,
    )?;

    Ok(PostbuildAnalyzeResult {
        metadata,
        remotes,
        server_nodes_plan,
    })
}
