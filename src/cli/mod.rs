mod common;
mod completions;
mod generate;
mod refresh;
mod verify;

use std::{env::current_dir, path::PathBuf, thread};

use clap::Parser;
use log::debug;
use simplelog::ColorChoice;

use crate::{
    checksum::{ChecksumAlgorithm, ChecksumMode, DEFAULT_CHUNK_SIZE},
    manifest::ManifestFormat,
};

use common::GlobalFlags;

impl GlobalFlags {
    /// Merge global flags, preferring subcommand flags when they are explicitly set
    pub fn merge(&self, subcommand_flags: &GlobalFlags) -> GlobalFlags {
        GlobalFlags {
            // For verbosity, use the maximum of the two (so -vv at top level and -v at subcommand would result in -vv)
            verbosity: std::cmp::max(self.verbosity, subcommand_flags.verbosity),
            // For boolean flags, OR them together (so either place can enable the flag)
            debug: self.debug || subcommand_flags.debug,
            no_color: self.no_color || subcommand_flags.no_color,
            no_progress: self.no_progress || subcommand_flags.no_progress,
            no_display: self.no_display || subcommand_flags.no_display,
        }
    }
}

#[derive(Debug, clap::Parser)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Option<Commands>,
    #[command(flatten)]
    pub global_flags: GlobalFlags,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    #[clap(
        name = "generate",
        about = "Generate a manifest file for the given directory",
        long_about = r#"Generate a manifest file for the given directory.

This command will generate a manifest file for the given directory.
The manifest file will contain the checksums of all files in the directory and its subdirectories."#
    )]
    Generate {
        /// Pattern to match files against
        #[arg(value_parser = clap::value_parser!(PathBuf), default_value = ".")]
        dirpath: PathBuf,
        #[arg(short, long, default_value = None)]
        /// Path to output the manifest file to
        output: Option<PathBuf>,
        #[arg(short, long, default_value = None)]
        /// Algorithm to use for checksum calculation
        algorithm: Option<ChecksumAlgorithm>,
        /// Format of the manifest file
        #[arg(short, long, default_value = "artsum")]
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
        /// Include files that are ignored through VCS ignore files
        /// (e.g. .gitignore, .ignore)
        #[arg(long, default_value_t = false)]
        ignore_vcs: bool,
        /// Chunk size to use for generating checksums
        #[arg(short, long, default_value_t = DEFAULT_CHUNK_SIZE)]
        chunk_size: usize,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers")]
        max_workers: Option<usize>,
        #[command(flatten)]
        global_flags: GlobalFlags,
    },
    #[clap(
        name = "verify",
        about = "Verify the checksums of files in the given directory",
        long_about = r#"Verify the checksums of files in the given directory.

This command will verify the checksums of the files listed in the manifest file. d
If no explicit manifest file is provided, it will look for a manifest file in the directory."#
    )]
    Verify {
        /// Path to the directory containing the files to verify
        #[arg(value_parser = clap::value_parser!(PathBuf), default_value = ".")]
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
        #[command(flatten)]
        global_flags: GlobalFlags,
    },
    #[clap(
        name = "refresh",
        about = "Refresh a manifest file in the given directory",
        long_about = r#"Refresh a manifest file in the given directory

This command will recalculate and rewrite the checksums of the files listed in the manifest file.
If no explicit manifest file is provided, it will look for a manifest file in the directory."#
    )]
    Refresh {
        /// Path to the directory containing the files to refresh
        #[arg(value_parser = clap::value_parser!(PathBuf), default_value = ".")]
        dirpath: PathBuf,
        /// Path to the manifest file to refresh
        #[arg(short, long, value_parser = clap::value_parser!(PathBuf))]
        manifest: Option<PathBuf>,
        /// Chunk size to use for generating checksums
        #[arg(short, long, default_value_t = DEFAULT_CHUNK_SIZE)]
        chunk_size: usize,
        /// Maximum number of workers to use
        #[arg(short = 'x', long = "max-workers")]
        max_workers: Option<usize>,
        #[command(flatten)]
        global_flags: GlobalFlags,
    },
    #[clap(
        name = "completions",
        about = "Generate shell completions for popular shells",
        long_about = r#"Generate shell completions for popular shells.

This command will generate completions for the specified shell (bash, zsh, or fish).
You can either print them to stdout, save them to a file, or install them automatically."#
    )]
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
        /// Output file path (prints to stdout if not specified)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Automatically install completions to the appropriate location
        #[arg(short, long, default_value_t = false)]
        install: bool,
    },
}

pub async fn cli() -> anyhow::Result<()> {
    let args = Cli::parse();

    // Helper function to get merged global flags for each command
    let get_merged_flags = |subcommand_flags: &GlobalFlags| -> GlobalFlags {
        args.global_flags.merge(subcommand_flags)
    };

    let default_max_parallelism = thread::available_parallelism()?.get();

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
            ignore_vcs,
            chunk_size,
            max_workers,
            ref global_flags,
        }) => {
            let merged_flags = get_merged_flags(&global_flags);

            if merged_flags.debug {
                let mut debug_loggers: Vec<Box<dyn simplelog::SharedLogger>> =
                    vec![simplelog::WriteLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        std::fs::File::create(format!(
                            "{}_artsum.log",
                            chrono::Local::now().format("%FT%T")
                        ))?,
                    )];

                if !merged_flags.no_display {
                    debug_loggers.push(simplelog::TermLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        simplelog::TerminalMode::Mixed,
                        if merged_flags.no_color {
                            ColorChoice::Never
                        } else {
                            ColorChoice::Auto
                        },
                    ))
                }
                simplelog::CombinedLogger::init(debug_loggers).unwrap();
            }

            if merged_flags.no_color {
                colored::control::set_override(false);
            }

            generate::generate(generate::GenerateOptions {
                dirpath,
                output,
                algorithm,
                format,
                mode,
                glob,
                include,
                exclude,
                ignore_vcs,
                chunk_size,
                max_workers: max_workers.unwrap_or(default_max_parallelism),
                debug: merged_flags.debug,
                no_display: merged_flags.no_display || merged_flags.debug,
                no_progress: merged_flags.no_progress
                    || merged_flags.no_display
                    || merged_flags.debug,
                verbosity: merged_flags.verbosity,
            })
            .await?;
        }
        Some(Commands::Verify {
            dirpath,
            manifest,
            chunk_size,
            max_workers,
            ref global_flags,
        }) => {
            let merged_flags = get_merged_flags(&global_flags);

            if merged_flags.debug {
                let mut debug_loggers: Vec<Box<dyn simplelog::SharedLogger>> =
                    vec![simplelog::WriteLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        std::fs::File::create(format!(
                            "{}_artsum.log",
                            chrono::Local::now().format("%FT%T")
                        ))?,
                    )];

                if !merged_flags.no_display {
                    debug_loggers.push(simplelog::TermLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        simplelog::TerminalMode::Mixed,
                        if merged_flags.no_color {
                            ColorChoice::Never
                        } else {
                            ColorChoice::Auto
                        },
                    ))
                }
                simplelog::CombinedLogger::init(debug_loggers).unwrap();
            }

            if merged_flags.no_color {
                colored::control::set_override(false);
            }

            verify::verify(verify::VerifyOptions {
                dirpath,
                manifest,
                chunk_size,
                max_workers: max_workers.unwrap_or(default_max_parallelism),
                debug: merged_flags.debug,
                no_display: merged_flags.no_display || merged_flags.debug,
                no_progress: merged_flags.no_progress
                    || merged_flags.no_display
                    || merged_flags.debug,
                verbosity: merged_flags.verbosity,
            })
            .await?;
        }
        Some(Commands::Refresh {
            dirpath,
            manifest,
            chunk_size,
            max_workers,
            ref global_flags,
        }) => {
            let merged_flags = get_merged_flags(&global_flags);

            if merged_flags.debug {
                let mut debug_loggers: Vec<Box<dyn simplelog::SharedLogger>> =
                    vec![simplelog::WriteLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        std::fs::File::create(format!(
                            "{}_artsum.log",
                            chrono::Local::now().format("%FT%T")
                        ))?,
                    )];

                if !merged_flags.no_display {
                    debug_loggers.push(simplelog::TermLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        simplelog::TerminalMode::Mixed,
                        if merged_flags.no_color {
                            ColorChoice::Never
                        } else {
                            ColorChoice::Auto
                        },
                    ))
                }
                simplelog::CombinedLogger::init(debug_loggers).unwrap();
            }

            if merged_flags.no_color {
                colored::control::set_override(false);
            }

            refresh::refresh(refresh::RefreshOptions {
                dirpath,
                manifest,
                chunk_size,
                max_workers: max_workers.unwrap_or(default_max_parallelism),
                debug: merged_flags.debug,
                no_display: merged_flags.no_display || merged_flags.debug,
                no_progress: merged_flags.no_progress
                    || merged_flags.no_display
                    || merged_flags.debug,
                verbosity: merged_flags.verbosity,
            })
            .await?;
        }
        Some(Commands::Completions {
            shell,
            output,
            install,
        }) => {
            completions::completions(completions::CompletionsOptions {
                shell,
                output,
                install,
            })?;
        }
        None => {
            // For the default command (verify), use only the top-level flags
            if args.global_flags.debug {
                let mut debug_loggers: Vec<Box<dyn simplelog::SharedLogger>> =
                    vec![simplelog::WriteLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        std::fs::File::create(format!(
                            "{}_artsum.log",
                            chrono::Local::now().format("%FT%T")
                        ))?,
                    )];

                if !args.global_flags.no_display {
                    debug_loggers.push(simplelog::TermLogger::new(
                        simplelog::LevelFilter::Debug,
                        simplelog::Config::default(),
                        simplelog::TerminalMode::Mixed,
                        if args.global_flags.no_color {
                            ColorChoice::Never
                        } else {
                            ColorChoice::Auto
                        },
                    ))
                }
                simplelog::CombinedLogger::init(debug_loggers).unwrap();
            }

            if args.global_flags.no_color {
                colored::control::set_override(false);
            }

            verify::verify(verify::VerifyOptions {
                dirpath: current_dir().unwrap(),
                manifest: None,
                chunk_size: DEFAULT_CHUNK_SIZE,
                max_workers: default_max_parallelism,
                debug: args.global_flags.debug,
                no_display: args.global_flags.no_display || args.global_flags.debug,
                no_progress: args.global_flags.no_progress
                    || args.global_flags.no_display
                    || args.global_flags.debug,
                verbosity: args.global_flags.verbosity,
            })
            .await?
        }
    }

    Ok(())
}
