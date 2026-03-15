use std::fs;
use std::io::{Read, Write};

use camino::{Utf8Path, Utf8PathBuf};
use flate2::{Compression, write::GzEncoder};
use rayon::prelude::*;

use crate::{
    Error,
    env::reserved_identifiers,
    error::{AdaptError, Result},
};

const COMPRESSIBLE_EXTENSIONS: &[&str] = &[
    "html", "js", "mjs", "json", "css", "svg", "xml", "wasm", "txt",
];

pub fn has_server_instrumentation_file(out_dir: &Utf8Path) -> bool {
    out_dir
        .join("output")
        .join("server")
        .join("instrumentation.server.js")
        .is_file()
}

pub fn compress_directory(directory: &Utf8Path) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }

    let mut files = Vec::new();
    collect_compressible_files(directory, directory, &mut files)?;
    compress_files_in_parallel(&files)
}

fn compress_files_in_parallel(files: &[Utf8PathBuf]) -> Result<()> {
    files.par_iter().try_for_each(|file| {
        compress_file(&file, CompressionFormat::Gzip)?;
        compress_file(&file, CompressionFormat::Brotli)?;
        Ok(())
    })
}

pub fn instrument_entrypoint(
    entrypoint: &Utf8Path,
    instrumentation: &Utf8Path,
    start: Option<&Utf8Path>,
    exports: &[String],
) -> Result<()> {
    if !instrumentation.is_file() {
        return Err(AdaptError::MissingInstrumentationFile {
            path: instrumentation.to_string(),
        }
        .into());
    }

    if !entrypoint.is_file() {
        return Err(AdaptError::MissingEntrypointFile {
            path: entrypoint.to_string(),
        }
        .into());
    }

    let start = start.map(Utf8Path::to_path_buf).unwrap_or_else(|| {
        entrypoint
            .parent()
            .unwrap_or(Utf8Path::new(""))
            .join("start.js")
    });

    copy_file(entrypoint, &start)?;

    let entrypoint_map = Utf8PathBuf::from(format!("{entrypoint}.map"));
    if entrypoint_map.is_file() {
        let start_map = Utf8PathBuf::from(format!("{start}.map"));
        copy_file(&entrypoint_map, &start_map)?;
    }

    let relative_instrumentation = relative_posix(
        entrypoint.parent().unwrap_or(Utf8Path::new("")),
        instrumentation,
    );
    let relative_start = relative_posix(entrypoint.parent().unwrap_or(Utf8Path::new("")), &start);

    let facade = create_instrumentation_facade(&relative_instrumentation, &relative_start, exports);

    fs::remove_file(entrypoint)?;
    fs::write(entrypoint, facade)?;
    Ok(())
}

pub fn create_instrumentation_facade(
    instrumentation: &str,
    start: &str,
    exports: &[String],
) -> String {
    let import_instrumentation = format!("import './{instrumentation}';");

    let reserved = reserved_identifiers();
    let mut alias_index = 0usize;
    let mut aliases = std::collections::BTreeMap::<String, String>::new();

    for name in exports
        .iter()
        .filter(|name| reserved.contains(name.as_str()))
    {
        let mut alias = format!("_{alias_index}");
        alias_index += 1;
        while exports.iter().any(|candidate| candidate == &alias) {
            alias = format!("_{alias_index}");
            alias_index += 1;
        }
        aliases.insert(name.clone(), alias);
    }

    let mut import_statements = Vec::new();
    let mut export_statements = Vec::new();

    for name in exports {
        if let Some(alias) = aliases.get(name) {
            import_statements.push(format!("{name}: {alias}"));
            export_statements.push(format!("{alias} as {name}"));
        } else {
            import_statements.push(name.clone());
            export_statements.push(name.clone());
        }
    }

    let entrypoint_facade = [
        format!(
            "const {{ {} }} = await import('./{}');",
            import_statements.join(", "),
            start
        ),
        if export_statements.is_empty() {
            String::new()
        } else {
            format!("export {{ {} }};", export_statements.join(", "))
        },
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n");

    format!("{import_instrumentation}\n{entrypoint_facade}")
}

fn copy_file(from: &Utf8Path, to: &Utf8Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to)?;
    Ok(())
}

fn relative_posix(from_dir: &Utf8Path, to: &Utf8Path) -> String {
    let relative = pathdiff::diff_paths(to, from_dir)
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| to.to_path_buf());
    relative.as_str().replace('\\', "/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompressionFormat {
    Brotli,
    Gzip,
}

fn collect_compressible_files(
    root: &Utf8Path,
    directory: &Utf8Path,
    files: &mut Vec<Utf8PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_compressible_files(root, &path, files)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let Some(extension) = path.extension() else {
            continue;
        };

        if COMPRESSIBLE_EXTENSIONS.contains(&extension) {
            files.push(
                path.strip_prefix(root)
                    .map(|relative| root.join(relative))
                    .unwrap_or(path),
            );
        }
    }

    Ok(())
}

fn compress_file(file: &Utf8Path, format: CompressionFormat) -> Result<()> {
    let mut source = fs::File::open(file)?;
    let mut input = Vec::new();
    source.read_to_end(&mut input)?;

    let output_path = match format {
        CompressionFormat::Gzip => Utf8PathBuf::from(format!("{file}.gz")),
        CompressionFormat::Brotli => Utf8PathBuf::from(format!("{file}.br")),
    };

    let mut output = fs::File::create(output_path)?;
    match format {
        CompressionFormat::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(&input)?;
            output.write_all(&encoder.finish()?)?;
        }
        CompressionFormat::Brotli => {
            let mut encoder = brotli::CompressorWriter::new(Vec::new(), 4096, 11, 22);
            encoder.write_all(&input)?;
            let compressed = encoder.into_inner();
            output.write_all(&compressed)?;
        }
    }

    Ok(())
}
