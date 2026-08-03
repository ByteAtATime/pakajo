use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pakajo")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
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
pub(crate) struct InstallArgs {
    #[arg(long)]
    pub(super) json: bool,
    #[arg(long = "asdeps")]
    pub(super) as_deps: bool,
    #[arg(long = "approvals")]
    pub(super) approvals_b64: Option<String>,
    #[arg(long = "skip-review")]
    pub(super) skip_review: bool,
    pub(super) positionals: Vec<String>,
}

#[derive(Args)]
pub(crate) struct RemoveArgs {
    #[arg(long)]
    pub(super) json: bool,
    pub(super) positionals: Vec<String>,
}

#[derive(Args)]
pub(crate) struct UpgradeArgs {
    #[arg(long)]
    pub(super) json: bool,
    #[arg(long = "no-refresh")]
    pub(super) no_refresh: bool,
    #[arg(long = "repo-only")]
    pub(super) repo_only: bool,
    #[arg(long = "ignore")]
    pub(super) ignores: Vec<String>,
    #[arg(long = "skip-review")]
    pub(super) skip_review: bool,
    #[arg(long = "fingerprint")]
    pub(super) fingerprint: Option<String>,
}

#[derive(Args)]
pub(crate) struct SearchArgs {
    pub(super) query: Vec<String>,
}

#[derive(Args)]
pub(crate) struct CleanArgs {
    #[arg(short = 'r', long = "remove")]
    pub(super) remove: bool,
}
