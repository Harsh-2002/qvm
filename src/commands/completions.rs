//! `qvm completions <shell>` - print a completion script for the chosen shell.
//!
//! Install instructions printed to stderr; the script itself goes to stdout
//! so you can pipe it.

use crate::error::Result;
use clap::CommandFactory;
use clap_complete::{generate, Shell};
use std::io;

pub fn run<C: CommandFactory>(shell: Shell) -> Result<()> {
    let mut cmd = C::command();
    let bin = cmd.get_name().to_string();

    // Hint - written to stderr so `qvm completions bash > /etc/...` still works clean.
    eprintln!("# Generated qvm completions for {shell:?}.");
    eprintln!("# Install hints:");
    match shell {
        Shell::Bash => {
            eprintln!("#   qvm completions bash | sudo tee /etc/bash_completion.d/qvm > /dev/null");
            eprintln!("#   # or for one shell session: source <(qvm completions bash)");
        }
        Shell::Fish => {
            eprintln!("#   qvm completions fish > ~/.config/fish/completions/qvm.fish");
        }
        Shell::Zsh => {
            eprintln!("#   qvm completions zsh | sudo tee /usr/share/zsh/site-functions/_qvm > /dev/null");
            eprintln!("#   # or: qvm completions zsh > ~/.zfunc/_qvm && add to fpath");
        }
        _ => {
            eprintln!("#   Source the output in your shell startup file.");
        }
    }

    generate(shell, &mut cmd, bin, &mut io::stdout());
    Ok(())
}
