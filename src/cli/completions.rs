use std::{fs, io, path::PathBuf};

use anyhow::{Context, Result};
use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::Cli;

#[derive(Debug)]
pub struct CompletionsOptions {
    pub shell: Shell,
    pub output: Option<PathBuf>,
    pub install: bool,
}

/// Generate shell completions for the specified shell
pub fn completions(options: CompletionsOptions) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();

    if options.install {
        install_completions(&options.shell, &bin_name)?;
    } else {
        generate_completions(&options.shell, &mut cmd, &bin_name, options.output)?;
    }

    Ok(())
}

/// Generate completions and write to output (stdout or file)
fn generate_completions(
    shell: &Shell,
    cmd: &mut clap::Command,
    bin_name: &str,
    output: Option<PathBuf>,
) -> Result<()> {
    if let Some(output_path) = output {
        let mut file = fs::File::create(&output_path)
            .with_context(|| format!("Failed to create output file: {:?}", output_path))?;
        generate(*shell, cmd, bin_name, &mut file);
        println!("Completions written to: {}", output_path.display());
    } else {
        let mut stdout = io::stdout();
        generate(*shell, cmd, bin_name, &mut stdout);
    }

    Ok(())
}

/// Install completions to the appropriate shell configuration directory
fn install_completions(shell: &Shell, bin_name: &str) -> Result<()> {
    let home_dir = dirs::home_dir().context("Failed to determine home directory")?;

    let (install_path, instructions) = match shell {
        Shell::Bash => {
            let completion_dir = home_dir.join(".local/share/bash-completion/completions");
            fs::create_dir_all(&completion_dir)
                .with_context(|| format!("Failed to create directory: {:?}", completion_dir))?;

            let install_path = completion_dir.join(bin_name);
            let instructions = format!(
                "Bash completion installed to: {}\n\
                 To enable completions, restart your shell or run:\n  \
                 source {}",
                install_path.display(),
                install_path.display()
            );
            (install_path, instructions)
        }
        Shell::Zsh => {
            let completion_dir = home_dir.join(".zsh/completions");
            fs::create_dir_all(&completion_dir)
                .with_context(|| format!("Failed to create directory: {:?}", completion_dir))?;

            let install_path = completion_dir.join(format!("_{}", bin_name));
            let instructions = format!(
                "Zsh completion installed to: {}\n\
                 Make sure the following is in your ~/.zshrc:\n  \
                 fpath=(~/.zsh/completions $fpath)\n  \
                 autoload -Uz compinit && compinit\n\
                 Then restart your shell or run: exec zsh",
                install_path.display()
            );
            (install_path, instructions)
        }
        Shell::Fish => {
            let completion_dir = home_dir.join(".config/fish/completions");
            fs::create_dir_all(&completion_dir)
                .with_context(|| format!("Failed to create directory: {:?}", completion_dir))?;

            let install_path = completion_dir.join(format!("{}.fish", bin_name));
            let instructions = format!(
                "Fish completion installed to: {}\n\
                 Completions should be available immediately in new fish shells.",
                install_path.display()
            );
            (install_path, instructions)
        }
        _ => {
            anyhow::bail!("Installation not supported for {:?}. Use --output to generate completions manually.", shell);
        }
    };

    // Generate and write the completions
    let mut cmd = Cli::command();
    let mut file = fs::File::create(&install_path)
        .with_context(|| format!("Failed to create completion file: {:?}", install_path))?;

    generate(*shell, &mut cmd, bin_name, &mut file);
    println!("{}", instructions);
    Ok(())
}
