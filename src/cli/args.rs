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
    Clean(CleanArgs),
    AurSync,
    Gendb,
}

#[derive(Args)]
pub struct InstallArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long = "asdeps")]
    pub as_deps: bool,
    #[arg(long = "approvals")]
    pub approvals_b64: Option<String>,
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
    #[arg(long = "approvals")]
    pub approvals_b64: Option<String>,
}

#[derive(Args)]
pub struct SearchArgs {
    pub query: Vec<String>,
}

#[derive(Args)]
pub struct CleanArgs {
    #[arg(short = 'r', long = "remove")]
    pub remove: bool,
}
