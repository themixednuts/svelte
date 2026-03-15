use clap::{Parser, Subcommand, ValueEnum};
use miette::{IntoDiagnostic, Result};
use svelte_compiler::{
    CompileOptions, ErrorMode, ExperimentalOptions, FragmentStrategy, GenerateTarget, compile,
};

#[derive(Debug, Parser)]
#[command(name = "svelte")]
#[command(about = "Unified Svelte toolchain binary (Rust port)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Compile(CompileCommand),
    Lsp,
    Kit,
    Create,
    Fmt,
    Lint,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GenerateArg {
    Client,
    Server,
}

#[derive(Debug, Parser)]
struct CompileCommand {
    #[arg(short, long)]
    input: std::path::PathBuf,

    #[arg(long, value_enum, default_value_t = GenerateArg::Client)]
    generate: GenerateArg,

    #[arg(long)]
    filename: Option<String>,

    #[arg(long)]
    dev: bool,

    #[arg(long)]
    runes: bool,

    #[arg(long)]
    r#async: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Compile(args) => run_compile(args),
        Command::Lsp => {
            println!("lsp subcommand scaffolded; implementation follows compiler parity");
            Ok(())
        }
        Command::Kit => {
            println!("kit subcommand scaffolded; implementation follows compiler parity");
            Ok(())
        }
        Command::Create => {
            println!("create subcommand scaffolded; implementation follows compiler parity");
            Ok(())
        }
        Command::Fmt => {
            println!("fmt subcommand scaffolded; implementation follows compiler parity");
            Ok(())
        }
        Command::Lint => {
            println!("lint subcommand scaffolded; implementation follows compiler parity");
            Ok(())
        }
    }
}

fn run_compile(args: CompileCommand) -> Result<()> {
    let source = std::fs::read_to_string(&args.input).into_diagnostic()?;

    let options = CompileOptions {
        filename: args
            .filename
            .map(camino::Utf8PathBuf::from)
            .or_else(|| camino::Utf8PathBuf::from_path_buf(args.input.clone()).ok()),
        generate: match args.generate {
            GenerateArg::Client => GenerateTarget::Client,
            GenerateArg::Server => GenerateTarget::Server,
        },
        dev: args.dev,
        runes: args.runes.then_some(true),
        error_mode: ErrorMode::Error,
        fragments: FragmentStrategy::Html,
        experimental: ExperimentalOptions {
            r#async: args.r#async,
        },
        ..CompileOptions::default()
    };

    let result = compile(&source, options).into_diagnostic()?;
    println!("{}", result.js.code);
    Ok(())
}
