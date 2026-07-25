pub(super) struct InstallArgs {
    pub(super) json: bool,
    pub(super) as_deps: bool,
    pub(super) approvals_b64: Option<String>,
    pub(super) positionals: Vec<String>,
}

impl InstallArgs {
    pub(super) fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let mut json = false;
        let mut as_deps = false;
        let mut approvals_b64: Option<String> = None;
        let mut positionals: Vec<String> = Vec::new();
        while let Some(s) = args.next() {
            if s == "--json" {
                json = true;
            } else if s == "--asdeps" {
                as_deps = true;
            } else if s == "--approvals" {
                let v = args.next().unwrap_or_else(|| {
                    eprintln!("--approvals requires a value");
                    std::process::exit(2);
                });
                approvals_b64 = Some(v);
            } else if let Some(rest) = s.strip_prefix("--approvals=") {
                approvals_b64 = Some(rest.to_string());
            } else if s.starts_with('-') {
                eprintln!("unknown flag: {s}");
                std::process::exit(2);
            } else {
                positionals.push(s);
            }
        }
        InstallArgs {
            json,
            as_deps,
            approvals_b64,
            positionals,
        }
    }
}

pub(super) struct RemoveArgs {
    pub(super) json: bool,
    pub(super) positionals: Vec<String>,
}

impl RemoveArgs {
    pub(super) fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut json = false;
        let mut positionals: Vec<String> = Vec::new();
        for s in args {
            if s == "--json" {
                json = true;
            } else if s.starts_with('-') {
                eprintln!("unknown flag: {s}");
                std::process::exit(2);
            } else {
                positionals.push(s);
            }
        }
        RemoveArgs { json, positionals }
    }
}

pub(super) struct UpgradeArgs {
    pub(super) json: bool,
    pub(super) no_refresh: bool,
    pub(super) repo_only: bool,
    pub(super) ignores: Vec<String>,
}

impl UpgradeArgs {
    pub(super) fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let mut json = false;
        let mut no_refresh = false;
        let mut repo_only = false;
        let mut ignores: Vec<String> = Vec::new();
        while let Some(s) = args.next() {
            if s == "--json" {
                json = true;
            } else if s == "--no-refresh" {
                no_refresh = true;
            } else if s == "--repo-only" {
                repo_only = true;
            } else if s == "--ignore" {
                let v = args.next().unwrap_or_else(|| {
                    eprintln!("--ignore requires a value");
                    std::process::exit(2);
                });
                ignores.push(v);
            } else if let Some(rest) = s.strip_prefix("--ignore=") {
                ignores.push(rest.to_string());
            } else if s.starts_with('-') {
                eprintln!("unknown flag: {s}");
                std::process::exit(2);
            } else {
                eprintln!("usage: pakajo upgrade [--json] [--no-refresh]");
                std::process::exit(2);
            }
        }
        UpgradeArgs {
            json,
            no_refresh,
            repo_only,
            ignores,
        }
    }
}
