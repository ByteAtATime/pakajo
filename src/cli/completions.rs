use super::args::{Cli, Shell};
use clap::CommandFactory as _;

pub fn run(shell: Shell) -> anyhow::Result<()> {
    let generator = match shell {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Zsh => clap_complete::Shell::Zsh,
        Shell::Fish => clap_complete::Shell::Fish,
    };
    let mut cmd = Cli::command();
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(generator, &mut cmd, "pakajo", &mut buf);
    print!("{}", String::from_utf8_lossy(&buf));
    Ok(())
}
