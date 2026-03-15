use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use rayon::join;
use serde_json::{Map, Value};

use crate::{
    BuildData, BuilderServerMetadata, HashValue, PostbuildAnalyzeResult, PrerenderExecutionPlan,
    PreviewPlan, Result, ServiceWorkerBuildEntry, ServiceWorkerBuildInvocationPlan,
    ValidatedConfig, ViteModuleIds, analyze_postbuild, build_prerender_execution_plan,
    build_preview_plan, hash_values, service_worker_build_invocation_plan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteBuildOrchestrationPlan {
    pub out: String,
    pub is_rolldown: bool,
    pub hash_routing: bool,
    pub version_hash: String,
    pub module_ids: ViteModuleIds,
    pub preview: PreviewPlan,
    pub prerender: PrerenderExecutionPlan,
    pub postbuild: PostbuildAnalyzeResult,
    pub service_worker: Option<ServiceWorkerBuildInvocationPlan>,
}

#[derive(Debug)]
struct ViteBuildSubplans {
    preview: PreviewPlan,
    postbuild: PostbuildAnalyzeResult,
    prerender: PrerenderExecutionPlan,
    service_worker: Option<ServiceWorkerBuildInvocationPlan>,
}

pub fn build_vite_orchestration_plan(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    build_data: &BuildData<'_>,
    metadata: &BuilderServerMetadata,
    manifest_path: &str,
    service_worker_entries: &BTreeMap<String, ServiceWorkerBuildEntry>,
    public_files: &[String],
    prerendered_paths: &[String],
    public_env: &Map<String, Value>,
    env: &BTreeMap<String, String>,
    hash_routing: bool,
    is_rolldown: bool,
) -> Result<ViteBuildOrchestrationPlan> {
    let out = relative_path(cwd, &build_data.out_dir);
    let subplans = build_vite_subplans(
        cwd,
        config,
        build_data,
        metadata,
        manifest_path,
        service_worker_entries,
        public_files,
        prerendered_paths,
        public_env,
        env,
        hash_routing,
        is_rolldown,
        &out,
    )?;

    Ok(ViteBuildOrchestrationPlan {
        out: out.as_str().to_string(),
        is_rolldown,
        hash_routing,
        version_hash: hash_values([HashValue::Str(&config.kit.version.name)])?,
        module_ids: ViteModuleIds::for_src_root(relative_path(cwd, &config.kit.files.src)),
        preview: subplans.preview,
        prerender: subplans.prerender,
        postbuild: subplans.postbuild,
        service_worker: subplans.service_worker,
    })
}

fn build_vite_subplans(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    build_data: &BuildData<'_>,
    metadata: &BuilderServerMetadata,
    manifest_path: &str,
    service_worker_entries: &BTreeMap<String, ServiceWorkerBuildEntry>,
    public_files: &[String],
    prerendered_paths: &[String],
    public_env: &Map<String, Value>,
    env: &BTreeMap<String, String>,
    hash_routing: bool,
    is_rolldown: bool,
    out: &Utf8Path,
) -> Result<ViteBuildSubplans> {
    let kit_out_dir = relative_path(cwd, &config.kit.out_dir);
    let (preview, other_subplans) = join(
        || build_preview_subplan(&kit_out_dir, config),
        || -> Result<(PostbuildAnalyzeResult, PrerenderExecutionPlan, Option<ServiceWorkerBuildInvocationPlan>)> {
            let (postbuild, other_subplans) = join(
                || build_postbuild_subplan(cwd, config, build_data),
                || {
                    join(
                        || {
                            build_prerender_subplan(
                                cwd,
                                config,
                                metadata,
                                manifest_path,
                                env,
                                hash_routing,
                            )
                        },
                        || {
                            build_service_worker_subplan(
                                config,
                                build_data,
                                service_worker_entries,
                                public_files,
                                prerendered_paths,
                                public_env,
                                is_rolldown,
                                out,
                            )
                        },
                    )
                },
            );

            Ok((postbuild?, other_subplans.0?, other_subplans.1?))
        },
    );

    let (postbuild, prerender, service_worker) = other_subplans?;

    Ok(ViteBuildSubplans {
        preview,
        postbuild,
        prerender,
        service_worker,
    })
}

fn build_preview_subplan(kit_out_dir: &Utf8Path, config: &ValidatedConfig) -> PreviewPlan {
    build_preview_plan(
        kit_out_dir,
        &config.kit.paths.base,
        &config.kit.paths.assets,
        false,
    )
}

fn build_postbuild_subplan(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    build_data: &BuildData<'_>,
) -> Result<PostbuildAnalyzeResult> {
    analyze_postbuild(cwd, config, build_data, None, None, &BTreeMap::new(), None)
}

fn build_prerender_subplan(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    metadata: &BuilderServerMetadata,
    manifest_path: &str,
    env: &BTreeMap<String, String>,
    hash_routing: bool,
) -> Result<PrerenderExecutionPlan> {
    build_prerender_execution_plan(cwd, &config.kit, manifest_path, metadata, hash_routing, env)
}

fn build_service_worker_subplan(
    config: &ValidatedConfig,
    build_data: &BuildData<'_>,
    service_worker_entries: &BTreeMap<String, ServiceWorkerBuildEntry>,
    public_files: &[String],
    prerendered_paths: &[String],
    public_env: &Map<String, Value>,
    is_rolldown: bool,
    out: &Utf8Path,
) -> Result<Option<ServiceWorkerBuildInvocationPlan>> {
    build_data
        .service_worker
        .as_ref()
        .map(|_| {
            service_worker_build_invocation_plan(
                &config.kit,
                out.as_str(),
                service_worker_entries,
                public_files,
                prerendered_paths,
                &config.kit.version.name,
                public_env,
                is_rolldown,
            )
        })
        .transpose()
}

fn relative_path(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(cwd)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}
