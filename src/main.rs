mod checksum;
mod cli;
mod manifest;

use colored::Colorize;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match cli::cli().await {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("{}: {}", "Error".red().bold(), e);
            std::process::exit(1);
        }
    }
}
