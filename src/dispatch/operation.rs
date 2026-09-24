pub const MARKER: &str = "__dispatch";

const REMOVE: &str = "remove";
const INSTALL: &str = "install";
const UPGRADE_REPO: &str = "upgrade-repo";
const STREAM: &str = "--stream";
const AS_DEPS: &str = "--asdeps";
const REINSTALL: &str = "--reinstall";
const NO_REFRESH: &str = "--no-refresh";
const IGNORE: &str = "--ignore";
const APPROVALS_FILE: &str = "--approvals-file";
const PRECONFIRMED: &str = "--preconfirmed";
const INTERACTIVE: &str = "--interactive";

pub enum PrivilegedOperation {
    Remove {
        targets: Vec<String>,
        interactive: bool,
        approvals: Option<crate::dispatch::approvals::ApprovalsFile>,
    },
    Install {
        targets: Vec<String>,
        as_deps: bool,
        reinstall: bool,
        preconfirmed: bool,
        interactive: bool,
        approvals: Option<crate::dispatch::approvals::ApprovalsFile>,
    },
    UpgradeRepo {
        no_refresh: bool,
        ignores: Vec<String>,
        interactive: bool,
        approvals: Option<crate::dispatch::approvals::ApprovalsFile>,
    },
}

#[derive(Debug)]
pub struct BuildOperation {
    pub targets: Vec<String>,
    pub files: Vec<String>,
    pub as_deps: bool,
    pub reinstall: bool,
    pub no_check: bool,
    pub interactive: bool,
}

#[derive(Debug, PartialEq)]
pub enum ChildOperation {
    Remove {
        targets: Vec<String>,
        interactive: bool,
        approvals_path: Option<String>,
        stream: bool,
    },
    Install {
        targets: Vec<String>,
        as_deps: bool,
        reinstall: bool,
        preconfirmed: bool,
        interactive: bool,
        approvals_path: Option<String>,
        stream: bool,
    },
    UpgradeRepo {
        no_refresh: bool,
        ignores: Vec<String>,
        interactive: bool,
        approvals_path: Option<String>,
        stream: bool,
    },
}

impl PrivilegedOperation {
    pub(crate) fn wire_args(&self, approvals_path: Option<&str>) -> Vec<String> {
        match self {
            PrivilegedOperation::Remove {
                targets,
                interactive,
                ..
            } => {
                let mut argv = vec![REMOVE.to_string(), STREAM.to_string()];
                if *interactive {
                    argv.push(INTERACTIVE.to_string());
                }
                if let Some(path) = approvals_path {
                    argv.push(APPROVALS_FILE.to_string());
                    argv.push(path.to_string());
                }
                argv.extend(targets.iter().cloned());
                argv
            }
            PrivilegedOperation::Install {
                targets,
                as_deps,
                reinstall,
                preconfirmed,
                interactive,
                ..
            } => {
                let mut argv = vec![INSTALL.to_string(), STREAM.to_string()];
                if *interactive {
                    argv.push(INTERACTIVE.to_string());
                }
                if *as_deps {
                    argv.push(AS_DEPS.to_string());
                }
                if *reinstall {
                    argv.push(REINSTALL.to_string());
                }
                if *preconfirmed {
                    argv.push(PRECONFIRMED.to_string());
                }
                if let Some(path) = approvals_path {
                    argv.push(APPROVALS_FILE.to_string());
                    argv.push(path.to_string());
                }
                argv.extend(targets.iter().cloned());
                argv
            }
            PrivilegedOperation::UpgradeRepo {
                no_refresh,
                ignores,
                interactive,
                ..
            } => {
                let mut argv = vec![UPGRADE_REPO.to_string(), STREAM.to_string()];
                if *interactive {
                    argv.push(INTERACTIVE.to_string());
                }
                if *no_refresh {
                    argv.push(NO_REFRESH.to_string());
                }
                for name in ignores {
                    argv.push(IGNORE.to_string());
                    argv.push(name.clone());
                }
                if let Some(path) = approvals_path {
                    argv.push(APPROVALS_FILE.to_string());
                    argv.push(path.to_string());
                }
                argv
            }
        }
    }
}

impl ChildOperation {
    pub fn decode(argv: &[String]) -> Option<ChildOperation> {
        match argv.first().map(String::as_str) {
            Some(REMOVE) => decode_remove(&argv[1..]),
            Some(INSTALL) => decode_install(&argv[1..]),
            Some(UPGRADE_REPO) => decode_upgrade_repo(&argv[1..]),
            _ => None,
        }
    }
}

fn decode_remove(argv: &[String]) -> Option<ChildOperation> {
    let mut stream = false;
    let mut interactive = false;
    let mut approvals_path = None;
    let mut targets = Vec::new();
    let mut parts = argv.iter();
    while let Some(arg) = parts.next() {
        match arg.as_str() {
            STREAM => stream = true,
            INTERACTIVE => interactive = true,
            APPROVALS_FILE => approvals_path = Some(parts.next()?.clone()),
            _ => {
                if arg.starts_with('-') {
                    return None;
                }
                targets.push(arg.clone());
            }
        }
    }
    Some(ChildOperation::Remove {
        targets,
        interactive,
        approvals_path,
        stream,
    })
}

fn decode_install(argv: &[String]) -> Option<ChildOperation> {
    let mut stream = false;
    let mut as_deps = false;
    let mut reinstall = false;
    let mut preconfirmed = false;
    let mut interactive = false;
    let mut approvals_path = None;
    let mut targets = Vec::new();
    let mut parts = argv.iter();
    while let Some(arg) = parts.next() {
        match arg.as_str() {
            STREAM => stream = true,
            AS_DEPS => as_deps = true,
            REINSTALL => reinstall = true,
            PRECONFIRMED => preconfirmed = true,
            INTERACTIVE => interactive = true,
            APPROVALS_FILE => approvals_path = Some(parts.next()?.clone()),
            _ => {
                if arg.starts_with('-') {
                    return None;
                }
                targets.push(arg.clone());
            }
        }
    }
    Some(ChildOperation::Install {
        targets,
        as_deps,
        reinstall,
        preconfirmed,
        interactive,
        approvals_path,
        stream,
    })
}

fn decode_upgrade_repo(argv: &[String]) -> Option<ChildOperation> {
    let mut stream = false;
    let mut no_refresh = false;
    let mut interactive = false;
    let mut ignores = Vec::new();
    let mut approvals_path = None;
    let mut parts = argv.iter();
    while let Some(arg) = parts.next() {
        match arg.as_str() {
            STREAM => stream = true,
            NO_REFRESH => no_refresh = true,
            INTERACTIVE => interactive = true,
            IGNORE => ignores.push(parts.next()?.clone()),
            APPROVALS_FILE => approvals_path = Some(parts.next()?.clone()),
            _ => return None,
        }
    }
    Some(ChildOperation::UpgradeRepo {
        no_refresh,
        ignores,
        interactive,
        approvals_path,
        stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_wire_round_trip() {
        let operation = PrivilegedOperation::Remove {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            interactive: false,
            approvals: None,
        };
        let argv = operation.wire_args(None);
        assert_eq!(argv, ["remove", "--stream", "sl", "figlet"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Remove {
                targets: vec!["sl".to_string(), "figlet".to_string()],
                interactive: false,
                approvals_path: None,
                stream: true,
            })
        );
    }

    #[test]
    fn remove_approvals_wire_round_trip() {
        let operation = PrivilegedOperation::Remove {
            targets: vec!["sl".to_string()],
            interactive: false,
            approvals: None,
        };
        let argv = operation.wire_args(Some("/tmp/pakajo-approvals-1.json"));
        assert_eq!(
            argv,
            [
                "remove",
                "--stream",
                "--approvals-file",
                "/tmp/pakajo-approvals-1.json",
                "sl"
            ]
        );
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Remove {
                targets: vec!["sl".to_string()],
                interactive: false,
                approvals_path: Some("/tmp/pakajo-approvals-1.json".to_string()),
                stream: true,
            })
        );
        let trailing = vec![
            "remove".to_string(),
            "--stream".to_string(),
            "--approvals-file".to_string(),
        ];
        assert_eq!(ChildOperation::decode(&trailing), None);
    }

    #[test]
    fn remove_interactive_wire_round_trip() {
        for (interactive, expected) in [
            (false, vec!["remove", "--stream", "sl"]),
            (true, vec!["remove", "--stream", "--interactive", "sl"]),
        ] {
            let operation = PrivilegedOperation::Remove {
                targets: vec!["sl".to_string()],
                interactive,
                approvals: None,
            };
            let argv = operation.wire_args(None);
            assert_eq!(argv, expected);
            assert_eq!(
                ChildOperation::decode(&argv),
                Some(ChildOperation::Remove {
                    targets: vec!["sl".to_string()],
                    interactive,
                    approvals_path: None,
                    stream: true,
                })
            );
        }
    }

    #[test]
    fn install_full_wire_round_trip() {
        let operation = PrivilegedOperation::Install {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            as_deps: true,
            reinstall: false,
            preconfirmed: false,
            interactive: false,
            approvals: None,
        };
        let argv = operation.wire_args(Some("/tmp/pakajo-approvals-1.json"));
        assert_eq!(
            argv,
            [
                "install",
                "--stream",
                "--asdeps",
                "--approvals-file",
                "/tmp/pakajo-approvals-1.json",
                "sl",
                "figlet"
            ]
        );
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Install {
                targets: vec!["sl".to_string(), "figlet".to_string()],
                as_deps: true,
                reinstall: false,
                preconfirmed: false,
                interactive: false,
                approvals_path: Some("/tmp/pakajo-approvals-1.json".to_string()),
                stream: true,
            })
        );
    }

    #[test]
    fn install_flag_wire_round_trip() {
        for (as_deps, reinstall, preconfirmed, interactive, flag) in [
            (false, true, false, false, "--reinstall"),
            (false, false, true, false, "--preconfirmed"),
            (false, false, false, true, "--interactive"),
        ] {
            let operation = PrivilegedOperation::Install {
                targets: vec!["sl".to_string()],
                as_deps,
                reinstall,
                preconfirmed,
                interactive,
                approvals: None,
            };
            let argv = operation.wire_args(None);
            assert_eq!(argv, ["install", "--stream", flag, "sl"]);
            assert_eq!(
                ChildOperation::decode(&argv),
                Some(ChildOperation::Install {
                    targets: vec!["sl".to_string()],
                    as_deps,
                    reinstall,
                    preconfirmed,
                    interactive,
                    approvals_path: None,
                    stream: true,
                })
            );
        }
    }

    #[test]
    fn upgrade_repo_full_wire_round_trip() {
        let operation = PrivilegedOperation::UpgradeRepo {
            no_refresh: true,
            ignores: vec!["foo".to_string(), "bar".to_string()],
            interactive: true,
            approvals: None,
        };
        let argv = operation.wire_args(Some("/tmp/pakajo-approvals-1.json"));
        assert_eq!(
            argv,
            [
                "upgrade-repo",
                "--stream",
                "--interactive",
                "--no-refresh",
                "--ignore",
                "foo",
                "--ignore",
                "bar",
                "--approvals-file",
                "/tmp/pakajo-approvals-1.json",
            ]
        );
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::UpgradeRepo {
                no_refresh: true,
                ignores: vec!["foo".to_string(), "bar".to_string()],
                interactive: true,
                approvals_path: Some("/tmp/pakajo-approvals-1.json".to_string()),
                stream: true,
            })
        );
    }

    #[test]
    fn upgrade_repo_minimal_wire_round_trip() {
        let operation = PrivilegedOperation::UpgradeRepo {
            no_refresh: false,
            ignores: vec![],
            interactive: false,
            approvals: None,
        };
        let argv = operation.wire_args(None);
        assert_eq!(argv, ["upgrade-repo", "--stream"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::UpgradeRepo {
                no_refresh: false,
                ignores: vec![],
                interactive: false,
                approvals_path: None,
                stream: true,
            })
        );
    }

    #[test]
    fn decode_rejects_foreign_head() {
        assert_eq!(ChildOperation::decode(&["upgrade".to_string()]), None);
        assert_eq!(ChildOperation::decode(&[]), None);
    }

    #[test]
    fn remove_rejects_unknown_dash_flag() {
        for flag in ["--asdep", "-x"] {
            let argv = vec![
                "remove".to_string(),
                "--stream".to_string(),
                flag.to_string(),
                "sl".to_string(),
            ];
            assert_eq!(ChildOperation::decode(&argv), None, "flag {flag}");
        }
    }

    #[test]
    fn install_rejects_unknown_dash_flag() {
        for flag in ["--asdep", "-x"] {
            let argv = vec![
                "install".to_string(),
                "--stream".to_string(),
                flag.to_string(),
                "sl".to_string(),
            ];
            assert_eq!(ChildOperation::decode(&argv), None, "flag {flag}");
        }
    }
}
