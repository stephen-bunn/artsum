mod generate;
mod verify;

use std::{env::current_dir, path::PathBuf, thread};

use clap::Parser;
use log::debug;
use simplelog::ColorChoice;

use crate::{
    checksum::{ChecksumAlgorithm, ChecksumMode, DEFAULT_CHUNK_SIZE},
    manifest::ManifestFormat,
};

#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Option<Commands>,
    #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
    /// Verbosity level
    pub verbosity: u8,
    /// Enable debug output
    #[arg(long, default_value_t = false)]
    pub debug: bool,
    /// Disable color output
    #[arg(long, default_value_t = false)]
    pub no_color: bool,
    /// Disable progress output
    #[arg(long, default_value_t = false)]
    pub no_progress: bool,
    /// Disable display output
    #[arg(long, default_value_t = false)]
    pub no_display: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    /// Generate a new manifest
    Generate {
        /// Pattern to match files against
        #[arg(value_parser = clap::value_parser!(PathBuf))]
        dirpath: PathBuf,
        #[arg(short, long, default_value = None)]
        /// Path to output the manifest file to
        output: Option<PathBuf>,
        #[arg(short, long, default_value = None)]
        /// Algorithm to use for checksum calculation
        algorithm: Option<ChecksumAlgorithm>,
        /// Format of the manifest file
        #[arg(short, long, default_value = "sfv")]
        format: Option<ManifestFormat>,
        #[arg(short, long, default_value = "binary")]
        /// Checksum mode to use for generating checksums
        mode: Option<ChecksumMode>,
        #[arg(short, long, default_value = "**/*")]
        /// Glob pattern to filter files
        glob: Option<String>,
        /// Regex patterns of the file paths to include in the manifest
        #[arg(short, long, default_value = None)]
        include: Option<Vec<String>>,
        /// Regex patterns of the file paths to exclude from the manifest
        /// (overrides include patterns)
        #[arg(short, long, default_value = None)]
        exclude: Option<Vec<String>>,
        /// Chunk size to use for generating checksums
        #[arg(short, long, default_value_t = DEFAULT_CHUNK_SIZE)]
        chunk_size: usize,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers", default_value = "8")]
        max_workers: usize,
    },

    /// Verify a manifest file in the given directory
    Verify {
        /// Path to the directory containing the files to verify
        #[arg(value_parser = clap::value_parser!(PathBuf))]
        dirpath: PathBuf,
        /// Path to the manifest file to verify
        #[arg(short, long, value_parser = clap::value_parser!(PathBuf))]
        manifest: Option<PathBuf>,
        /// Chunk size to use for generating checksums
        #[arg(short, long, default_value_t = DEFAULT_CHUNK_SIZE)]
        chunk_size: usize,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers")]
        max_workers: Option<usize>,
    },
}

pub async fn cli() -> anyhow::Result<()> {
    let args = Cli::parse();

    if args.debug {
        let mut debug_loggers: Vec<Box<dyn simplelog::SharedLogger>> =
            vec![simplelog::WriteLogger::new(
                simplelog::LevelFilter::Debug,
                simplelog::Config::default(),
                std::fs::File::create(format!("{}_sfv.log", chrono::Local::now().format("%FT%T")))?,
            )];

        if !args.no_display {
            debug_loggers.push(simplelog::TermLogger::new(
                simplelog::LevelFilter::Debug,
                simplelog::Config::default(),
                simplelog::TerminalMode::Mixed,
                if args.no_color {
                    ColorChoice::Never
                } else {
                    ColorChoice::Auto
                },
            ))
        }
        simplelog::CombinedLogger::init(debug_loggers).unwrap();
    }

    if args.no_color {
        colored::control::set_override(false);
    }

    debug!("{:?}", args);
    match args.command {
        Some(Commands::Generate {
            dirpath,
            output,
            algorithm,
            format,
            mode,
            glob,
            include,
            exclude,
            chunk_size,
            max_workers,
        }) => {
            generate::generate(generate::GenerateOptions {
                dirpath,
                output,
                algorithm,
                format,
                mode,
                glob,
                include,
                exclude,
                chunk_size,
                max_workers,
                debug: args.debug,
                no_display: args.no_display || args.debug,
                no_progress: args.no_progress || args.no_display || args.debug,
                verbosity: args.verbosity,
            })
            .await?;
        }
        Some(Commands::Verify {
            dirpath,
            manifest,
            chunk_size,
            max_workers,
        }) => {
            verify::verify(verify::VerifyOptions {
                dirpath,
                manifest,
                chunk_size,
                max_workers: max_workers.unwrap_or(thread::available_parallelism()?.get()),
                debug: args.debug,
                no_display: args.no_display || args.debug,
                no_progress: args.no_progress || args.no_display || args.debug,
                verbosity: args.verbosity,
            })
            .await?;
        }
        None => {
            verify::verify(verify::VerifyOptions {
                dirpath: current_dir().unwrap(),
                manifest: None,
                chunk_size: DEFAULT_CHUNK_SIZE,
                max_workers: thread::available_parallelism()?.get(),
                debug: args.debug,
                no_display: args.no_display || args.debug,
                no_progress: args.no_progress || args.no_display || args.debug,
                verbosity: args.verbosity,
            })
            .await?
        }
    }

    Ok(())
}
