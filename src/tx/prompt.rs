use std::cell::RefCell;
use std::io::{BufRead, Write};

use crate::color;
use crate::question::model::{Answer, Question};
use crate::question::source::{
    AnswerSource, FailClosed, SourceDecision, parse_group_selection, parse_provider_selection,
};
use crate::tx::driver::{self, RunOutcome, RunSpec};

pub fn render(question: &Question, colored: bool) -> String {
    match question {
        Question::Conflict {
            incoming,
            removable,
        } => colon_text(
            colored,
            &format!("{incoming} and {removable} are in conflict. Remove {removable}? [y/N] "),
        ),
        Question::SelectProvider { depend, candidates } => {
            render_provider(depend, candidates, colored)
        }
        Question::Replace { old, new, repo } => render_replace(old, new, repo.as_deref(), colored),
        Question::InstallIgnorepkg { name } => render_ignorepkg(name, colored),
        Question::RemovePkgs { names } => render_remove_pkgs(names, colored),
        Question::Corrupted { path } => render_corrupted(path, colored),
        Question::ImportKey { fingerprint, uid } => render_import_key(fingerprint, uid, colored),
        Question::Proceed(summary) => render_proceed(summary, colored),
        Question::GroupMembers { group, members } => render_group(group, members, colored),
    }
}

fn colon_text(colored: bool, text: &str) -> String {
    if colored {
        color::colon(true, text)
    } else {
        text.to_string()
    }
}

fn render_provider(
    depend: &str,
    candidates: &[crate::question::model::ProviderCandidate],
    colored: bool,
) -> String {
    let mut out = match candidates.len() {
        1 => colon_text(
            colored,
            &format!("There is 1 provider available for {depend}:\n"),
        ),
        count => colon_text(
            colored,
            &format!("There are {count} providers available for {depend}:\n"),
        ),
    };
    for (index, candidate) in candidates.iter().enumerate() {
        match &candidate.repo {
            Some(repo) => out.push_str(&format!("  [{}] {repo}/{}\n", index + 1, candidate.name)),
            None => out.push_str(&format!("  [{}] {}\n", index + 1, candidate.name)),
        }
    }
    out.push_str("\nEnter a number (default=1): ");
    out
}

fn render_replace(old: &str, new: &str, repo: Option<&str>, colored: bool) -> String {
    match repo {
        Some(repo) => colon_text(colored, &format!("Replace {old} with {repo}/{new}? [Y/n] ")),
        None => colon_text(colored, &format!("Replace {old} with {new}? [Y/n] ")),
    }
}

fn render_ignorepkg(name: &str, colored: bool) -> String {
    colon_text(
        colored,
        &format!("{name} is in IgnorePkg/IgnoreGroup. Install anyway? [Y/n] "),
    )
}

fn render_remove_pkgs(names: &[String], colored: bool) -> String {
    let single = names.len() == 1;
    let package_word = if single { "package" } else { "packages" };
    let mut out = colon_text(
        colored,
        &format!(
            "The following {package_word} cannot be upgraded due to unresolvable dependencies:\n"
        ),
    );
    for name in names {
        out.push_str(&format!("  {name}\n"));
    }
    out.push_str(&colon_text(
        colored,
        &format!("Do you want to skip the above {package_word} for this upgrade? [y/N] "),
    ));
    out
}

fn render_corrupted(path: &str, colored: bool) -> String {
    colon_text(
        colored,
        &format!("File {path} is corrupted. Do you want to delete it? [Y/n] "),
    )
}

fn render_import_key(fingerprint: &str, uid: &str, colored: bool) -> String {
    if uid.is_empty() {
        colon_text(colored, &format!("Import PGP key {fingerprint}? [Y/n] "))
    } else {
        colon_text(
            colored,
            &format!("Import PGP key {fingerprint}, \"{uid}\"? [Y/n] "),
        )
    }
}

fn render_proceed(_summary: &crate::events::TransactionSummary, colored: bool) -> String {
    colon_text(colored, "Proceed with installation? [Y/n] ")
}

fn render_group(group: &str, members: &[String], colored: bool) -> String {
    let mut out = colon_text(
        colored,
        &format!("There are {} members in group {group}:\n", members.len()),
    );
    for (index, member) in members.iter().enumerate() {
        out.push_str(&format!("  {}) {member}\n", index + 1));
    }
    out.push_str("\nEnter a selection (default=all): ");
    out
}

fn confirm(line: &str, default_yes: bool) -> bool {
    match line.trim().to_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

fn decide(question: &Question, line: &str) -> Option<Answer> {
    match question {
        Question::Conflict {
            incoming,
            removable,
        } => Some(Answer::Conflict {
            incoming: incoming.clone(),
            removable: removable.clone(),
            remove: confirm(line, false),
        }),
        Question::SelectProvider { candidates, .. } => {
            let chosen = parse_provider_selection(line, candidates.len())?;
            let candidate = candidates.get(chosen - 1)?;
            Some(Answer::SelectProvider {
                name: candidate.name.clone(),
                repo: candidate.repo.clone(),
            })
        }
        Question::Replace { old, new, .. } => Some(Answer::Replace {
            old: old.clone(),
            new: new.clone(),
            replace: confirm(line, true),
        }),
        Question::InstallIgnorepkg { name } => Some(Answer::InstallIgnorepkg {
            name: name.clone(),
            install: confirm(line, true),
        }),
        Question::RemovePkgs { names } => Some(Answer::RemovePkgs {
            names: names.clone(),
            skip: confirm(line, false),
        }),
        Question::Corrupted { path } => Some(Answer::Corrupted {
            path: path.clone(),
            remove: confirm(line, true),
        }),
        Question::ImportKey { fingerprint, .. } => Some(Answer::ImportKey {
            fingerprint: fingerprint.clone(),
            import: confirm(line, true),
        }),
        Question::Proceed(_) => {
            if confirm(line, true) {
                Some(Answer::Proceed)
            } else {
                None
            }
        }
        Question::GroupMembers { members, .. } => {
            let picked = parse_group_selection(line, members.len())?;
            Some(Answer::GroupMembers {
                selected: picked
                    .into_iter()
                    .filter_map(|n| members.get(n - 1).cloned())
                    .collect(),
            })
        }
    }
}

pub struct InteractiveSource<R, W> {
    input: RefCell<R>,
    output: RefCell<W>,
    colored: bool,
}

impl<R: BufRead, W: Write> InteractiveSource<R, W> {
    pub fn new(input: R, output: W, colored: bool) -> Self {
        Self {
            input: RefCell::new(input),
            output: RefCell::new(output),
            colored,
        }
    }

    pub fn into_parts(self) -> (R, W) {
        (self.input.into_inner(), self.output.into_inner())
    }

    fn prompt(&self, question: &Question) -> Result<String, String> {
        let text = render(question, self.colored);
        let (hide, show) = if self.colored {
            (color::HIDE_CURSOR, color::SHOW_CURSOR)
        } else {
            ("", "")
        };
        {
            let mut output = self.output.borrow_mut();
            let mut send = |bytes: &str| {
                output
                    .write_all(bytes.as_bytes())
                    .map_err(|_| "failed to write prompt".to_string())
            };
            send(hide)?;
            send(&text)?;
            send(show)?;
            output
                .flush()
                .map_err(|_| "failed to write prompt".to_string())?;
        }
        let mut line = String::new();
        let read = self.input.borrow_mut().read_line(&mut line);
        let mut output = self.output.borrow_mut();
        let rehide = output.write_all(hide.as_bytes());
        let flushed = rehide.and_then(|_| output.flush());
        match read {
            Ok(0) => Err("end of input".to_string()),
            Ok(_) if flushed.is_err() => Err("failed to write prompt".to_string()),
            Ok(_) => Ok(line),
            Err(_) => Err("failed to read answer".to_string()),
        }
    }
}

impl<R: BufRead, W: Write> AnswerSource for InteractiveSource<R, W> {
    fn answer(&self, question: &Question) -> SourceDecision {
        let line = match self.prompt(question) {
            Ok(line) => line,
            Err(reason) => {
                return SourceDecision::Abort(FailClosed {
                    key: question.key(),
                    reason,
                });
            }
        };
        match decide(question, &line) {
            Some(answer) => SourceDecision::Answer(answer),
            None => SourceDecision::Answer(Answer::Stop),
        }
    }
}

pub fn with_preapproved_proceed(inner: Box<dyn AnswerSource>) -> Box<dyn AnswerSource> {
    Box::new(PreapprovedProceed { inner })
}

struct PreapprovedProceed {
    inner: Box<dyn AnswerSource>,
}

impl AnswerSource for PreapprovedProceed {
    fn answer(&self, question: &Question) -> SourceDecision {
        match question {
            Question::Proceed(_) => SourceDecision::Answer(Answer::Proceed),
            other => self.inner.answer(other),
        }
    }
}

pub fn execute_with<R: BufRead + 'static, W: Write + 'static>(
    input: R,
    output: W,
    colored: bool,
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    sink: Box<dyn crate::events::InstallSink>,
) -> anyhow::Result<RunOutcome> {
    let source = InteractiveSource::new(input, output, colored);
    execute_with_source(Box::new(source), handle, spec, sink)
}

pub fn execute_with_source(
    source: Box<dyn AnswerSource>,
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    sink: Box<dyn crate::events::InstallSink>,
) -> anyhow::Result<RunOutcome> {
    driver::run(handle, spec, source, sink)
}

pub fn execute(
    handle: &mut alpm::Alpm,
    spec: &RunSpec,
    sink: Box<dyn crate::events::InstallSink>,
) -> anyhow::Result<RunOutcome> {
    execute_with(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
        color::stdout_color(),
        handle,
        spec,
        sink,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::TransactionSummary;
    use crate::question::model::{ProviderCandidate, QuestionKey};
    use std::io::Cursor;

    fn summary() -> TransactionSummary {
        TransactionSummary {
            packages: vec![],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 0,
        }
    }

    fn candidate(name: &str, repo: &str) -> ProviderCandidate {
        ProviderCandidate {
            name: name.to_string(),
            repo: Some(repo.to_string()),
            version: None,
        }
    }

    fn ask(question: &Question, input: &[u8], colored: bool) -> (SourceDecision, String) {
        let source = InteractiveSource::new(Cursor::new(input.to_vec()), Vec::new(), colored);
        let decision = source.answer(question);
        let (_, output) = source.into_parts();
        (decision, String::from_utf8(output).unwrap())
    }

    fn conflict() -> Question {
        Question::Conflict {
            incoming: "newpkg".to_string(),
            removable: "oldpkg".to_string(),
        }
    }

    struct Script;

    impl AnswerSource for Script {
        fn answer(&self, _question: &Question) -> SourceDecision {
            SourceDecision::Answer(Answer::Stop)
        }
    }

    #[test]
    fn preapproved_proceed_answers_proceed_and_delegates_rest() {
        let source = with_preapproved_proceed(Box::new(Script));
        assert!(matches!(
            source.answer(&Question::Proceed(summary())),
            SourceDecision::Answer(Answer::Proceed)
        ));
        assert!(matches!(
            source.answer(&conflict()),
            SourceDecision::Answer(Answer::Stop)
        ));
    }

    #[test]
    fn prompt_wording_matches_pacman() {
        let cases: Vec<(Question, &str)> = vec![
            (
                conflict(),
                "newpkg and oldpkg are in conflict. Remove oldpkg? [y/N] ",
            ),
            (
                Question::SelectProvider {
                    depend: "sdl".to_string(),
                    candidates: vec![candidate("foo", "core"), candidate("bar", "extra")],
                },
                "There are 2 providers available for sdl:\n  [1] core/foo\n  [2] extra/bar\n\nEnter a number (default=1): ",
            ),
            (
                Question::SelectProvider {
                    depend: "libgl".to_string(),
                    candidates: vec![candidate("libglvnd", "extra")],
                },
                "There is 1 provider available for libgl:\n  [1] extra/libglvnd\n\nEnter a number (default=1): ",
            ),
            (
                Question::GroupMembers {
                    group: "tools".to_string(),
                    members: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                },
                "There are 3 members in group tools:\n  1) a\n  2) b\n  3) c\n\nEnter a selection (default=all): ",
            ),
            (
                Question::Proceed(summary()),
                "Proceed with installation? [Y/n] ",
            ),
            (
                Question::Corrupted {
                    path: "/var/cache/pkg-1.pkg.tar.zst".to_string(),
                },
                "File /var/cache/pkg-1.pkg.tar.zst is corrupted. Do you want to delete it? [Y/n] ",
            ),
            (
                Question::ImportKey {
                    fingerprint: "ABCDEF".to_string(),
                    uid: "Packager <pack@example.com>".to_string(),
                },
                "Import PGP key ABCDEF, \"Packager <pack@example.com>\"? [Y/n] ",
            ),
            (
                Question::ImportKey {
                    fingerprint: "ABCDEF".to_string(),
                    uid: String::new(),
                },
                "Import PGP key ABCDEF? [Y/n] ",
            ),
            (
                Question::InstallIgnorepkg {
                    name: "glibc".to_string(),
                },
                "glibc is in IgnorePkg/IgnoreGroup. Install anyway? [Y/n] ",
            ),
            (
                Question::Replace {
                    old: "nginx".to_string(),
                    new: "nginx-mainline".to_string(),
                    repo: Some("extra".to_string()),
                },
                "Replace nginx with extra/nginx-mainline? [Y/n] ",
            ),
            (
                Question::RemovePkgs {
                    names: vec!["a".to_string(), "b".to_string()],
                },
                "The following packages cannot be upgraded due to unresolvable dependencies:\n  a\n  b\nDo you want to skip the above packages for this upgrade? [y/N] ",
            ),
        ];
        for (question, expected) in cases {
            assert_eq!(render(&question, false), expected);
        }
    }

    #[test]
    fn selection_answers_follow_rules() {
        let provider = Question::SelectProvider {
            depend: "sdl".to_string(),
            candidates: vec![candidate("foo", "core"), candidate("bar", "extra")],
        };
        let (decision, prompt) = ask(&provider, b"\n", false);
        assert_eq!(
            prompt,
            "There are 2 providers available for sdl:\n  [1] core/foo\n  [2] extra/bar\n\nEnter a number (default=1): "
        );
        match decision {
            SourceDecision::Answer(Answer::SelectProvider { name, repo }) => {
                assert_eq!(name, "foo");
                assert_eq!(repo, Some("core".to_string()));
            }
            other => panic!("expected provider answer, got {other:?}"),
        }
        match ask(&provider, b"2\n", false).0 {
            SourceDecision::Answer(Answer::SelectProvider { name, repo }) => {
                assert_eq!(name, "bar");
                assert_eq!(repo, Some("extra".to_string()));
            }
            other => panic!("expected provider answer, got {other:?}"),
        }
        let single_provider = Question::SelectProvider {
            depend: "sdl".to_string(),
            candidates: vec![candidate("foo", "core")],
        };
        assert!(matches!(
            ask(&single_provider, b"9\n", false).0,
            SourceDecision::Answer(Answer::Stop)
        ));

        let group = Question::GroupMembers {
            group: "tools".to_string(),
            members: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        };
        match ask(&group, b"\n", false).0 {
            SourceDecision::Answer(Answer::GroupMembers { selected }) => {
                assert_eq!(selected, vec!["a", "b", "c"]);
            }
            other => panic!("expected group answer, got {other:?}"),
        }
        match ask(&group, b"1 3\n", false).0 {
            SourceDecision::Answer(Answer::GroupMembers { selected }) => {
                assert_eq!(selected, vec!["a", "c"]);
            }
            other => panic!("expected group answer, got {other:?}"),
        }
        let single_group = Question::GroupMembers {
            group: "tools".to_string(),
            members: vec!["a".to_string()],
        };
        assert!(matches!(
            ask(&single_group, b"99\n", false).0,
            SourceDecision::Answer(Answer::Stop)
        ));
    }

    #[test]
    fn confirm_answers_follow_presented_defaults() {
        match ask(&conflict(), b"y\n", false).0 {
            SourceDecision::Answer(Answer::Conflict { remove, .. }) => assert!(remove),
            other => panic!("expected conflict answer, got {other:?}"),
        }
        match ask(&conflict(), b"\n", false).0 {
            SourceDecision::Answer(Answer::Conflict { remove, .. }) => assert!(!remove),
            other => panic!("expected conflict answer, got {other:?}"),
        }
        assert!(matches!(
            ask(&Question::Proceed(summary()), b"\n", false).0,
            SourceDecision::Answer(Answer::Proceed)
        ));
        assert!(matches!(
            ask(&Question::Proceed(summary()), b"n\n", false).0,
            SourceDecision::Answer(Answer::Stop)
        ));
        for (question, check) in [
            (
                Question::Replace {
                    old: "nginx".to_string(),
                    new: "nginx-mainline".to_string(),
                    repo: Some("extra".to_string()),
                },
                "replace",
            ),
            (
                Question::InstallIgnorepkg {
                    name: "glibc".to_string(),
                },
                "ignorepkg",
            ),
            (
                Question::Corrupted {
                    path: "/var/cache/pkg-1.pkg.tar.zst".to_string(),
                },
                "corrupted",
            ),
            (
                Question::ImportKey {
                    fingerprint: "ABCDEF".to_string(),
                    uid: "Packager <pack@example.com>".to_string(),
                },
                "import",
            ),
        ] {
            assert!(
                matches!(ask(&question, b"\n", false).0, SourceDecision::Answer(_)),
                "empty input accepts default-yes {check}",
            );
        }
    }

    #[test]
    fn eof_aborts_with_question_key() {
        let question = Question::GroupMembers {
            group: "tools".to_string(),
            members: vec!["a".to_string()],
        };
        let (decision, prompt) = ask(&question, b"", false);
        assert_eq!(
            prompt,
            "There are 1 members in group tools:\n  1) a\n\nEnter a selection (default=all): "
        );
        match decision {
            SourceDecision::Abort(denied) => {
                assert_eq!(
                    denied.key,
                    QuestionKey::GroupMembers {
                        group: "tools".to_string(),
                    }
                );
                assert_eq!(denied.reason, "end of input");
            }
            other => panic!("expected abort, got {other:?}"),
        }
    }
}
