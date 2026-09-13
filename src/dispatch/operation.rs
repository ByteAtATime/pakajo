pub const MARKER: &str = "__dispatch";

const REMOVE: &str = "remove";
const INSTALL: &str = "install";
const UPGRADE_REPO: &str = "upgrade-repo";
const STREAM: &str = "--stream";
const AS_DEPS: &str = "--asdeps";
const NO_REFRESH: &str = "--no-refresh";
const IGNORE: &str = "--ignore";
const FINGERPRINT_FILE: &str = "--fingerprint-file";
const APPROVALS_FILE: &str = "--approvals-file";

pub enum PrivilegedOperation {
    Remove {
        targets: Vec<String>,
    },
    Install {
        targets: Vec<String>,
        as_deps: bool,
        approvals: Option<String>,
    },
    UpgradeRepo {
        no_refresh: bool,
        ignores: Vec<String>,
        fingerprint: Option<crate::dispatch::approvals::ApprovalsFile>,
        approvals: Option<String>,
    },
}

#[derive(Debug)]
pub struct BuildOperation {
    pub targets: Vec<String>,
    pub as_deps: bool,
}

#[derive(Debug, PartialEq)]
pub enum ChildOperation {
    Remove {
        targets: Vec<String>,
        stream: bool,
    },
    Install {
        targets: Vec<String>,
        as_deps: bool,
        approvals_path: Option<String>,
        stream: bool,
    },
    UpgradeRepo {
        no_refresh: bool,
        ignores: Vec<String>,
        fingerprint_path: Option<String>,
        approvals_path: Option<String>,
        stream: bool,
    },
}

impl PrivilegedOperation {
    pub(crate) fn wire_args(
        &self,
        approvals_path: Option<&str>,
        fingerprint_path: Option<&str>,
    ) -> Vec<String> {
        match self {
            PrivilegedOperation::Remove { targets } => {
                let mut argv = vec![REMOVE.to_string(), STREAM.to_string()];
                argv.extend(targets.iter().cloned());
                argv
            }
            PrivilegedOperation::Install {
                targets, as_deps, ..
            } => {
                let mut argv = vec![INSTALL.to_string(), STREAM.to_string()];
                if *as_deps {
                    argv.push(AS_DEPS.to_string());
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
                ..
            } => {
                let mut argv = vec![UPGRADE_REPO.to_string(), STREAM.to_string()];
                if *no_refresh {
                    argv.push(NO_REFRESH.to_string());
                }
                for name in ignores {
                    argv.push(IGNORE.to_string());
                    argv.push(name.clone());
                }
                if let Some(path) = fingerprint_path {
                    argv.push(FINGERPRINT_FILE.to_string());
                    argv.push(path.to_string());
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
    let mut targets = Vec::new();
    for arg in argv {
        if arg.as_str() == STREAM {
            stream = true;
        } else {
            targets.push(arg.clone());
        }
    }
    Some(ChildOperation::Remove { targets, stream })
}

fn decode_install(argv: &[String]) -> Option<ChildOperation> {
    let mut stream = false;
    let mut as_deps = false;
    let mut approvals_path = None;
    let mut targets = Vec::new();
    let mut parts = argv.iter();
    while let Some(arg) = parts.next() {
        match arg.as_str() {
            STREAM => stream = true,
            AS_DEPS => as_deps = true,
            APPROVALS_FILE => approvals_path = Some(parts.next()?.clone()),
            _ => targets.push(arg.clone()),
        }
    }
    Some(ChildOperation::Install {
        targets,
        as_deps,
        approvals_path,
        stream,
    })
}

fn decode_upgrade_repo(argv: &[String]) -> Option<ChildOperation> {
    let mut stream = false;
    let mut no_refresh = false;
    let mut ignores = Vec::new();
    let mut fingerprint_path = None;
    let mut approvals_path = None;
    let mut parts = argv.iter();
    while let Some(arg) = parts.next() {
        match arg.as_str() {
            STREAM => stream = true,
            NO_REFRESH => no_refresh = true,
            IGNORE => ignores.push(parts.next()?.clone()),
            FINGERPRINT_FILE => fingerprint_path = Some(parts.next()?.clone()),
            APPROVALS_FILE => approvals_path = Some(parts.next()?.clone()),
            _ => return None,
        }
    }
    Some(ChildOperation::UpgradeRepo {
        no_refresh,
        ignores,
        fingerprint_path,
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
        };
        let argv = operation.wire_args(None, None);
        assert_eq!(argv, ["remove", "--stream", "sl", "figlet"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Remove {
                targets: vec!["sl".to_string(), "figlet".to_string()],
                stream: true,
            })
        );
    }

    #[test]
    fn install_full_wire_round_trip() {
        let operation = PrivilegedOperation::Install {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            as_deps: true,
            approvals: None,
        };
        let argv = operation.wire_args(Some("/tmp/pakajo-approvals-1.json"), None);
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
                approvals_path: Some("/tmp/pakajo-approvals-1.json".to_string()),
                stream: true,
            })
        );
    }

    #[test]
    fn install_minimal_wire_round_trip() {
        let operation = PrivilegedOperation::Install {
            targets: vec!["sl".to_string()],
            as_deps: false,
            approvals: None,
        };
        let argv = operation.wire_args(None, None);
        assert_eq!(argv, ["install", "--stream", "sl"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Install {
                targets: vec!["sl".to_string()],
                as_deps: false,
                approvals_path: None,
                stream: true,
            })
        );
    }

    #[test]
    fn install_as_deps_wire_round_trip() {
        let operation = PrivilegedOperation::Install {
            targets: vec!["sl".to_string()],
            as_deps: true,
            approvals: None,
        };
        let argv = operation.wire_args(None, None);
        assert_eq!(argv, ["install", "--stream", "--asdeps", "sl"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::Install {
                targets: vec!["sl".to_string()],
                as_deps: true,
                approvals_path: None,
                stream: true,
            })
        );
    }

    #[test]
    fn upgrade_repo_full_wire_round_trip() {
        let operation = PrivilegedOperation::UpgradeRepo {
            no_refresh: true,
            ignores: vec!["foo".to_string(), "bar".to_string()],
            fingerprint: None,
            approvals: None,
        };
        let argv = operation.wire_args(Some("/tmp/pakajo-approvals-1.json"), Some("/tmp/fp.json"));
        assert_eq!(
            argv,
            [
                "upgrade-repo",
                "--stream",
                "--no-refresh",
                "--ignore",
                "foo",
                "--ignore",
                "bar",
                "--fingerprint-file",
                "/tmp/fp.json",
                "--approvals-file",
                "/tmp/pakajo-approvals-1.json",
            ]
        );
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::UpgradeRepo {
                no_refresh: true,
                ignores: vec!["foo".to_string(), "bar".to_string()],
                fingerprint_path: Some("/tmp/fp.json".to_string()),
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
            fingerprint: None,
            approvals: None,
        };
        let argv = operation.wire_args(None, None);
        assert_eq!(argv, ["upgrade-repo", "--stream"]);
        assert_eq!(
            ChildOperation::decode(&argv),
            Some(ChildOperation::UpgradeRepo {
                no_refresh: false,
                ignores: vec![],
                fingerprint_path: None,
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
}
