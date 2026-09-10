pub const MARKER: &str = "__dispatch";

const REMOVE: &str = "remove";
const STREAM: &str = "--stream";

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    Remove { targets: Vec<String>, stream: bool },
}

impl Operation {
    pub fn encode(&self) -> Vec<String> {
        let Operation::Remove { targets, stream } = self;
        let mut argv = vec![REMOVE.to_string()];
        if *stream {
            argv.push(STREAM.to_string());
        }
        argv.extend(targets.iter().cloned());
        argv
    }

    pub fn decode(argv: &[String]) -> Option<Operation> {
        let mut parts = argv.iter();
        if parts.next().map(String::as_str) != Some(REMOVE) {
            return None;
        }
        let mut stream = false;
        let mut targets = Vec::new();
        for arg in parts {
            if arg.as_str() == STREAM {
                stream = true;
            } else {
                targets.push(arg.clone());
            }
        }
        Some(Operation::Remove { targets, stream })
    }
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
    fn decode_rejects_foreign_head() {
        assert_eq!(Operation::decode(&["install".to_string()]), None);
        assert_eq!(Operation::decode(&[]), None);
    }
}
