use clap::{Arg, ArgAction, Command};
use clap_mangen::Man;
use std::fs;
use std::path::PathBuf;

// Version should match the artsum crate version
const ARTSUM_VERSION: &str = "0.2.0";

fn build_cli() -> Command {
    Command::new("artsum")
        .version(ARTSUM_VERSION)
        .about("A simple command-line tool for generating and verifying a directory manifest of checksums")
        .author("Stephen Bunn <stephen@bunn.io>")
        .arg(
            Arg::new("verbosity")
                .short('v')
                .long("verbosity")
                .action(ArgAction::Count)
                .help("Verbosity level")
                .global(true),
        )
        .arg(
            Arg::new("debug")
                .long("debug")
                .action(ArgAction::SetTrue)
                .help("Enable debug output")
                .global(true),
        )
        .arg(
            Arg::new("no-color")
                .long("no-color")
                .action(ArgAction::SetTrue)
                .help("Disable color output")
                .global(true),
        )
        .arg(
            Arg::new("no-progress")
                .long("no-progress")
                .action(ArgAction::SetTrue)
                .help("Disable progress output")
                .global(true),
        )
        .arg(
            Arg::new("no-display")
                .long("no-display")
                .action(ArgAction::SetTrue)
                .help("Disable display output")
                .global(true),
        )
        .subcommand(
            Command::new("generate")
                .about("Generate a manifest file for the given directory")
                .long_about("Generate a manifest file for the given directory.\n\nThis command will generate a manifest file for the given directory.\nThe manifest file will contain the checksums of all files in the directory and its subdirectories.")
                .arg(
                    Arg::new("dirpath")
                        .help("Pattern to match files against")
                        .default_value(".")
                        .index(1),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Path to output the manifest file to"),
                )
                .arg(
                    Arg::new("algorithm")
                        .short('a')
                        .long("algorithm")
                        .help("Algorithm to use for checksum calculation"),
                )
                .arg(
                    Arg::new("format")
                        .short('f')
                        .long("format")
                        .help("Format of the manifest file")
                        .default_value("artsum"),
                )
                .arg(
                    Arg::new("mode")
                        .short('m')
                        .long("mode")
                        .help("Checksum mode to use for generating checksums")
                        .default_value("binary"),
                )
                .arg(
                    Arg::new("glob")
                        .short('g')
                        .long("glob")
                        .help("Glob pattern to filter files")
                        .default_value("**/*"),
                )
                .arg(
                    Arg::new("include")
                        .short('i')
                        .long("include")
                        .help("Regex patterns of the file paths to include in the manifest")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("exclude")
                        .short('e')
                        .long("exclude")
                        .help("Regex patterns of the file paths to exclude from the manifest (overrides include patterns)")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("ignore-vcs")
                        .long("ignore-vcs")
                        .action(ArgAction::SetTrue)
                        .help("Include files that are ignored through VCS ignore files (e.g. .gitignore, .ignore)"),
                )
                .arg(
                    Arg::new("chunk-size")
                        .short('c')
                        .long("chunk-size")
                        .help("Chunk size to use for generating checksums"),
                )
                .arg(
                    Arg::new("max-workers")
                        .short('x')
                        .long("max-workers")
                        .help("Maximum number of workers to use"),
                ),
        )
        .subcommand(
            Command::new("verify")
                .about("Verify the checksums of files in the given directory")
                .long_about("Verify the checksums of files in the given directory.\n\nThis command will verify the checksums of the files listed in the manifest file.\nIf no explicit manifest file is provided, it will look for a manifest file in the directory.")
                .arg(
                    Arg::new("dirpath")
                        .help("Path to the directory containing the files to verify")
                        .default_value(".")
                        .index(1),
                )
                .arg(
                    Arg::new("manifest")
                        .short('m')
                        .long("manifest")
                        .help("Path to the manifest file to verify"),
                )
                .arg(
                    Arg::new("chunk-size")
                        .short('c')
                        .long("chunk-size")
                        .help("Chunk size to use for generating checksums"),
                )
                .arg(
                    Arg::new("max-workers")
                        .short('x')
                        .long("max-workers")
                        .help("Maximum number of workers to use"),
                ),
        )
        .subcommand(
            Command::new("refresh")
                .about("Refresh a manifest file in the given directory")
                .long_about("Refresh a manifest file in the given directory\n\nThis command will recalculate and rewrite the checksums of the files listed in the manifest file.\nIf no explicit manifest file is provided, it will look for a manifest file in the directory.")
                .arg(
                    Arg::new("dirpath")
                        .help("Path to the directory containing the files to refresh")
                        .default_value(".")
                        .index(1),
                )
                .arg(
                    Arg::new("manifest")
                        .short('m')
                        .long("manifest")
                        .help("Path to the manifest file to refresh"),
                )
                .arg(
                    Arg::new("chunk-size")
                        .short('c')
                        .long("chunk-size")
                        .help("Chunk size to use for generating checksums"),
                )
                .arg(
                    Arg::new("max-workers")
                        .short('x')
                        .long("max-workers")
                        .help("Maximum number of workers to use"),
                ),
        )
}

fn generate_man_page(out_dir: &PathBuf) -> anyhow::Result<()> {
    fs::create_dir_all(out_dir)?;

    let cmd = build_cli();
    
    // Generate main man page
    let man = Man::new(cmd.clone());
    let mut buffer: Vec<u8> = Vec::new();
    man.render(&mut buffer)?;
    let man_path = out_dir.join("artsum.1");
    fs::write(&man_path, buffer)?;
    println!("Man page generated at {:?}", man_path);

    // Generate man pages for each subcommand
    let subcommands = cmd.get_subcommands().filter(|cmd| cmd.get_name() != "help");
    
    for subcmd in subcommands {
        let name = subcmd.get_name();
        let man = Man::new(subcmd.clone());
        let mut buffer: Vec<u8> = Vec::new();
        man.render(&mut buffer)?;
        
        let man_path = out_dir.join(format!("artsum-{}.1", name));
        fs::write(&man_path, buffer)?;
        println!("Man page generated at {:?}", man_path);
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: cargo xtask <COMMAND>");
        eprintln!("\nCommands:");
        eprintln!("  man    Generate the man page");
        std::process::exit(1);
    }

    match args[1].as_str() {
        "man" => {
            // Default to target/man directory
            let out_dir = PathBuf::from(
                std::env::var("OUT_DIR").unwrap_or_else(|_| "target/man".to_string())
            );
            generate_man_page(&out_dir)?;
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            eprintln!("\nAvailable commands:");
            eprintln!("  man    Generate the man page");
            std::process::exit(1);
        }
    }

    Ok(())
}
