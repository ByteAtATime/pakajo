use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pakajo")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(alias = "add")]
    Install(InstallArgs),
    #[command(alias = "uninstall", alias = "rm")]
    Remove(RemoveArgs),
    Upgrade(UpgradeArgs),
    Search(SearchArgs),
    Info(InfoArgs),
    Clean(CleanArgs),
    AurSync,
    Gendb,
    Completions(CompletionsArgs),
}

#[derive(Args)]
pub struct InstallArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long = "asdeps")]
    pub as_deps: bool,
    #[arg(long = "reinstall")]
    pub reinstall: bool,
    #[arg(long = "skip-review")]
    pub skip_review: bool,
    pub positionals: Vec<String>,
}

#[derive(Args)]
pub struct RemoveArgs {
    #[arg(long)]
    pub json: bool,
    pub positionals: Vec<String>,
}

#[derive(Args)]
pub struct UpgradeArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long = "no-refresh")]
    pub no_refresh: bool,
    #[arg(long = "repo-only")]
    pub repo_only: bool,
    #[arg(long = "ignore")]
    pub ignores: Vec<String>,
    #[arg(long = "skip-review")]
    pub skip_review: bool,
    #[arg(long = "fingerprint-file")]
    pub fingerprint_file: Option<String>,
}

#[derive(Args)]
pub struct SearchArgs {
    pub query: Vec<String>,
}

#[derive(Args)]
pub struct InfoArgs {
    #[arg(required = true, num_args = 1..)]
    pub targets: Vec<String>,
}

#[derive(Args)]
pub struct CleanArgs {
    #[arg(short = 'r', long = "remove")]
    pub remove: bool,
}

#[derive(Args)]
pub struct CompletionsArgs {
    pub shell: Shell,
}

#[derive(clap::ValueEnum, Clone)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}
