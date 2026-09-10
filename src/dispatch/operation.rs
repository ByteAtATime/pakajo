pub const MARKER: &str = "__dispatch";

const REMOVE: &str = "remove";
const INSTALL: &str = "install";
const STREAM: &str = "--stream";
const AS_DEPS: &str = "--asdeps";
const APPROVALS_FILE: &str = "--approvals-file";

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
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
}

impl Operation {
    pub fn encode(&self) -> Vec<String> {
        match self {
            Operation::Remove { targets, stream } => {
                let mut argv = vec![REMOVE.to_string()];
                if *stream {
                    argv.push(STREAM.to_string());
                }
                argv.extend(targets.iter().cloned());
                argv
            }
            Operation::Install {
                targets,
                as_deps,
                approvals_path,
                stream,
            } => {
                let mut argv = vec![INSTALL.to_string()];
                if *stream {
                    argv.push(STREAM.to_string());
                }
                if *as_deps {
                    argv.push(AS_DEPS.to_string());
                }
                if let Some(path) = approvals_path {
                    argv.push(APPROVALS_FILE.to_string());
                    argv.push(path.clone());
                }
                argv.extend(targets.iter().cloned());
                argv
            }
        }
    }

    pub fn decode(argv: &[String]) -> Option<Operation> {
        match argv.first().map(String::as_str) {
            Some(REMOVE) => decode_remove(&argv[1..]),
            Some(INSTALL) => decode_install(&argv[1..]),
            _ => None,
        }
    }
}

fn decode_remove(argv: &[String]) -> Option<Operation> {
    let mut stream = false;
    let mut targets = Vec::new();
    for arg in argv {
        if arg.as_str() == STREAM {
            stream = true;
        } else {
            targets.push(arg.clone());
        }
    }
    Some(Operation::Remove { targets, stream })
}

fn decode_install(argv: &[String]) -> Option<Operation> {
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
    Some(Operation::Install {
        targets,
        as_deps,
        approvals_path,
        stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_streaming_round_trip() {
        let operation = Operation::Remove {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            stream: true,
        };
        assert_eq!(operation.encode(), ["remove", "--stream", "sl", "figlet"]);
        assert_eq!(Operation::decode(&operation.encode()), Some(operation));
    }

    #[test]
    fn remove_plain_round_trip() {
        let operation = Operation::Remove {
            targets: vec!["sl".to_string()],
            stream: false,
        };
        assert_eq!(operation.encode(), ["remove", "sl"]);
        assert_eq!(Operation::decode(&operation.encode()), Some(operation));
    }

    #[test]
    fn install_full_round_trip() {
        let operation = Operation::Install {
            targets: vec!["sl".to_string(), "figlet".to_string()],
            as_deps: true,
            approvals_path: Some("/tmp/pakajo-approvals-1.json".to_string()),
            stream: true,
        };
        assert_eq!(
            operation.encode(),
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
        assert_eq!(Operation::decode(&operation.encode()), Some(operation));
    }

    #[test]
    fn install_minimal_round_trip() {
        let operation = Operation::Install {
            targets: vec!["sl".to_string()],
            as_deps: false,
            approvals_path: None,
            stream: false,
        };
        assert_eq!(operation.encode(), ["install", "sl"]);
        assert_eq!(Operation::decode(&operation.encode()), Some(operation));
    }

    #[test]
    fn install_as_deps_round_trip() {
        let operation = Operation::Install {
            targets: vec!["sl".to_string()],
            as_deps: true,
            approvals_path: None,
            stream: true,
        };
        assert_eq!(
            operation.encode(),
            ["install", "--stream", "--asdeps", "sl"]
        );
        assert_eq!(Operation::decode(&operation.encode()), Some(operation));
    }

    #[test]
    fn decode_rejects_foreign_head() {
        assert_eq!(Operation::decode(&["upgrade".to_string()]), None);
        assert_eq!(Operation::decode(&[]), None);
    }
}
