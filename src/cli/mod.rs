mod generate;
mod verify;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{checksum::ChecksumAlgorithm, manifest::ManifestFormat};

#[derive(Debug, Parser)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Option<Commands>,
    #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
    /// Verbosity level
    pub verbosity: u8,
    /// Disable color output
    #[arg(long, default_value_t = false)]
    pub no_color: bool,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Generate a new manifest
    Generate {
        /// Pattern to match files against
        #[arg(value_parser = clap::value_parser!(PathBuf))]
        dirpath: PathBuf,
        #[arg(short, long, default_value = None)]
        /// Path to output the manifest file to
        output: Option<PathBuf>,
        #[arg(short, long, default_value = "xxh3")]
        /// Algorithm to use for checksum calculation
        algorithm: Option<ChecksumAlgorithm>,
        /// Format of the manifest file
        #[arg(short, long, default_value = "sfv")]
        format: Option<ManifestFormat>,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers", default_value = "8")]
        max_workers: usize,
    },

    /// Verify a manifest file in the given directory
    Verify {
        /// Path to the directory containing the manifest file
        #[arg(value_parser = clap::value_parser!(PathBuf))]
        dirpath: PathBuf,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers", default_value = "8")]
        max_workers: usize,
    },
}

pub async fn cli() -> anyhow::Result<()> {
    let args = Cli::parse();
    if args.no_color {
        colored::control::set_override(false);
    }

    match args.command {
        Some(Commands::Generate {
            dirpath,
            output,
            algorithm,
            format,
            max_workers,
        }) => {
            generate::generate(generate::GenerateOptions {
                dirpath,
                output,
                algorithm,
                format,
                max_workers,
                verbosity: args.verbosity,
            })
            .await?;
        }
        Some(Commands::Verify {
            dirpath,
            max_workers,
        }) => {
            verify::verify(verify::VerifyOptions {
                dirpath,
                max_workers,
                verbosity: args.verbosity,
            })
            .await?;
        }
        None => {}
    }

    Ok(())
}
