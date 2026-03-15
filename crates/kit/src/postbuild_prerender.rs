use std::{collections::BTreeMap, fs};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    BuilderPrerenderOption, BuilderServerMetadata, Error, Result, ValidatedKitConfig,
    filesystem::posixify,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackGenerationPlan {
    pub output_root: Utf8PathBuf,
    pub manifest_path: Utf8PathBuf,
    pub server_internal_path: Utf8PathBuf,
    pub server_index_path: Utf8PathBuf,
    pub assets_dir: Utf8PathBuf,
    pub request_path: String,
    pub request_url: String,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderExecutionPlan {
    pub hash: bool,
    pub output_root: Utf8PathBuf,
    pub manifest_path: Utf8PathBuf,
    pub server_internal_path: Utf8PathBuf,
    pub server_index_path: Utf8PathBuf,
    pub client_dir: Utf8PathBuf,
    pub prerendered_pages_dir: Utf8PathBuf,
    pub prerendered_dependencies_dir: Utf8PathBuf,
    pub origin: String,
    pub entries: Vec<String>,
    pub crawl: bool,
    pub concurrency: u64,
    pub remote_prefix: String,
    pub static_files: Vec<String>,
    pub server_immutable_files: Vec<String>,
    pub prerender_map: BTreeMap<String, BuilderPrerenderOption>,
    pub fallback: Option<FallbackGenerationPlan>,
}

pub fn build_fallback_generation_plan(
    cwd: &Utf8Path,
    kit: &ValidatedKitConfig,
    manifest_path: &str,
    env: &BTreeMap<String, String>,
) -> FallbackGenerationPlan {
    let manifest_path = Utf8PathBuf::from(manifest_path);
    let output_root = manifest_path
        .parent()
        .and_then(Utf8Path::parent)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|| relative_path(cwd, &kit.out_dir).join("output"));

    FallbackGenerationPlan {
        output_root: output_root.clone(),
        manifest_path: manifest_path.clone(),
        server_internal_path: output_root.join("server/internal.js"),
        server_index_path: output_root.join("server/index.js"),
        assets_dir: relative_path(cwd, &kit.files.assets),
        request_path: "/[fallback]".to_string(),
        request_url: format!("{}/[fallback]", kit.prerender.origin.trim_end_matches('/')),
        env: env.clone(),
    }
}

pub fn build_prerender_execution_plan(
    cwd: &Utf8Path,
    kit: &ValidatedKitConfig,
    manifest_path: &str,
    metadata: &BuilderServerMetadata,
    hash: bool,
    env: &BTreeMap<String, String>,
) -> Result<PrerenderExecutionPlan> {
    let out_dir = relative_path(cwd, &kit.out_dir);
    let output_root = out_dir.join("output");
    let client_dir = output_root.join("client");
    let server_dir = output_root.join("server");
    let prerendered_root = output_root.join("prerendered");

    let mut static_files = collect_relative_files(&cwd.join(&client_dir))?;
    static_files.push(format!("{}/env.js", kit.app_dir));
    static_files.sort();
    static_files.dedup();

    let server_immutable_root = cwd.join(&server_dir).join(&kit.app_dir).join("immutable");
    let server_immutable_files = if server_immutable_root.exists() {
        collect_relative_files(&server_immutable_root)?
            .into_iter()
            .map(|path| format!("{}/immutable/{path}", kit.app_dir))
            .collect()
    } else {
        Vec::new()
    };

    let prerender_map = metadata
        .routes
        .iter()
        .filter_map(|(id, route)| route.prerender.clone().map(|value| (id.clone(), value)))
        .collect();

    Ok(PrerenderExecutionPlan {
        hash,
        output_root: output_root.clone(),
        manifest_path: Utf8PathBuf::from(manifest_path),
        server_internal_path: server_dir.join("internal.js"),
        server_index_path: server_dir.join("index.js"),
        client_dir,
        prerendered_pages_dir: prerendered_root.join("pages"),
        prerendered_dependencies_dir: prerendered_root.join("dependencies"),
        origin: kit.prerender.origin.clone(),
        entries: kit.prerender.entries.clone(),
        crawl: kit.prerender.crawl,
        concurrency: kit.prerender.concurrency,
        remote_prefix: format!("{}/{}/remote/", kit.paths.base, kit.app_dir).replace("//", "/"),
        static_files,
        server_immutable_files,
        prerender_map,
        fallback: hash.then(|| build_fallback_generation_plan(cwd, kit, manifest_path, env)),
    })
}

fn relative_path(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(cwd)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn collect_relative_files(root: &Utf8Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }

    fn walk(root: &Utf8Path, current: &Utf8Path, files: &mut Vec<String>) -> Result<()> {
        let metadata = fs::metadata(current)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(current)? {
                let entry = entry?;
                let path =
                    Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
                walk(root, &path, files)?;
            }
            return Ok(());
        }

        let relative = current
            .strip_prefix(root)
            .expect("walked file should stay under the root");
        files.push(posixify(relative.as_str()));
        Ok(())
    }

    walk(root, root, &mut files)?;
    files.sort();
    Ok(files)
}
