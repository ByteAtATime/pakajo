use crate::cli::{ConsoleSink, privs};
use crate::cli::{EscalatedSink, JsonSink, answerer_for, confirm_remove, confirm_remove_stderr};
use crate::dispatch::operation::Operation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovePresentation {
    InteractiveStream,
    SilentStream,
    Console,
}

pub fn select_remove_presentation(stream: bool, tty: bool) -> RemovePresentation {
    if stream {
        if tty {
            RemovePresentation::InteractiveStream
        } else {
            RemovePresentation::SilentStream
        }
    } else {
        RemovePresentation::Console
    }
}

pub fn run(argv: &[String]) -> ! {
    let Some(Operation::Remove { targets, stream }) = Operation::decode(argv) else {
        eprintln!("malformed dispatch argv");
        std::process::exit(1);
    };
    run_remove_root(&targets, stream);
}

pub fn run_remove_root(targets: &[String], stream: bool) -> ! {
    match select_remove_presentation(stream, privs::stdin_is_tty()) {
        RemovePresentation::InteractiveStream => exit_with_result(crate::remove::run_remove(
            targets,
            EscalatedSink::new(),
            confirm_remove_stderr,
            answerer_for(None),
        )),
        RemovePresentation::SilentStream => exit_with_result(crate::remove::run_remove(
            targets,
            JsonSink::new(),
            || true,
            answerer_for(None),
        )),
        RemovePresentation::Console => exit_with_result(crate::remove::run_remove(
            targets,
            ConsoleSink::new(),
            confirm_remove,
            answerer_for(None),
        )),
    }
}

fn exit_with_result(result: anyhow::Result<()>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_presentation_keys_on_terminal_alone() {
        use RemovePresentation::{Console, InteractiveStream, SilentStream};
        let cases = [
            ((true, true), InteractiveStream),
            ((true, false), SilentStream),
            ((false, true), Console),
            ((false, false), Console),
        ];
        for ((stream, tty), expected) in cases {
            assert_eq!(
                select_remove_presentation(stream, tty),
                expected,
                "select_remove_presentation(stream={stream}, tty={tty})"
            );
        }
    }
}
