use pakajo::db::PackageDb;
use pakajo::resolve::{Base, Conflict, Decisions, Engine, Plan};
use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

const STATIC_TARGETS: [&str; 17] = [
    "yay-bin",
    "google-chrome",
    "visual-studio-code-bin",
    "brave-bin",
    "spotify",
    "foxitreader",
    "glibc",
    "vim",
    "neovim",
    "java-environment",
    "nvidia-470xx-dkms",
    "base-devel",
    "this-package-does-not-exist-xyz123",
    "pakajo-fuzz-no-such-pkg-xyz",
    "yay",
    "zfs-dkms",
    "wordnet-common",
];

const DYNAMIC_CANDIDATES: i64 = 50;
const DYNAMIC_TOP_N: usize = 20;
const PAIR_COUNT: usize = 8;
const TRIPLE_COUNT: usize = 4;
const FIXTURE: &str = "[options]\nProvides = yes\n";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fuzz-paru.conf")
}

fn log_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fuzz-resolve.log")
}

fn pkg_role(target: bool, make: bool) -> &'static str {
    if target {
        return "TARGET";
    }
    if make {
        return "MAKE";
    }
    "DEP"
}

fn render_conflict(kind: &str, conflict: &Conflict) -> Vec<String> {
    conflict
        .conflicting
        .iter()
        .map(|entry| match &entry.conflict {
            Some(detail) => {
                format!("CONFLICT {kind} {} {} {detail}", conflict.pkg, entry.pkg)
            }
            None => format!("CONFLICT {kind} {} {}", conflict.pkg, entry.pkg),
        })
        .collect()
}

fn render_plan(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    for missing in &plan.missing {
        let mut line = format!("MISSING {}", missing.dep);
        for frame in &missing.stack {
            line.push(' ');
            line.push_str(&frame.pkg);
        }
        lines.push(line);
    }
    for conflict in &plan.conflicts.local {
        lines.extend(render_conflict("LOCAL", conflict));
    }
    for conflict in &plan.conflicts.inner {
        lines.extend(render_conflict("INNER", conflict));
    }
    for install in &plan.repo_installs {
        lines.push(format!(
            "REPO {} {} {}",
            pkg_role(install.target, install.make),
            install.db,
            install.name
        ));
    }
    for base in &plan.bases {
        match base {
            Base::Aur { base, members, .. } => {
                for member in members {
                    lines.push(format!(
                        "AUR {} {base} {}",
                        pkg_role(member.target, member.make),
                        member.name
                    ));
                }
            }
            Base::Pkgbuild { .. } => {
                panic!("this should be unreachable i think");
            }
        }
    }
    lines
}

fn collapse_oracle_duplicates(lines: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(lines.len());
    for line in lines {
        let duplicate = kept
            .last()
            .is_some_and(|prior| prior == line && line.starts_with("CONFLICT "));
        if !duplicate {
            kept.push(line.clone());
        }
    }
    kept
}

fn load_dynamic_targets() -> Vec<String> {
    let path = PackageDb::db_path().unwrap();
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = db
        .prepare("SELECT name FROM packages WHERE source = 'aur' ORDER BY num_votes DESC LIMIT ?1")
        .unwrap();
    let names: Vec<String> = statement
        .query_map(rusqlite::params![DYNAMIC_CANDIDATES], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<String>, _>>()
        .unwrap();
    let statics: HashSet<&str> = STATIC_TARGETS.iter().copied().collect();
    names
        .into_iter()
        .filter(|name| !statics.contains(name.as_str()))
        .take(DYNAMIC_TOP_N)
        .collect()
}

fn combo_cases(statics: &[String]) -> Vec<Vec<String>> {
    let mut cases = Vec::new();
    let count = statics.len();
    for index in 0..PAIR_COUNT {
        let first = index % count;
        let mut second = (index * 7 + 3) % count;
        if second == first {
            second = (second + 1) % count;
        }
        cases.push(vec![statics[first].clone(), statics[second].clone()]);
    }
    for index in 0..TRIPLE_COUNT {
        let mut picks = [
            index % count,
            (index * 5 + 2) % count,
            (index * 11 + 6) % count,
        ];
        for slot in 1..3 {
            while picks[slot] == picks[0] || (slot == 2 && picks[slot] == picks[1]) {
                picks[slot] = (picks[slot] + 1) % count;
            }
        }
        cases.push(picks.iter().map(|slot| statics[*slot].clone()).collect());
    }
    cases.push(vec![
        "nvidia-470xx-utils".to_string(),
        "nvidia-utils".to_string(),
    ]);
    cases
}

fn run_oracle(fixture: &PathBuf, targets: &[String]) -> (Vec<String>, String) {
    let output = Command::new("paru")
        .arg("-P")
        .arg("--order")
        .args(targets)
        .env("PARU_CONF", fixture)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (lines, stderr)
}

fn run_engine(targets: &[String]) -> Vec<String> {
    let mut engine = Engine::new(false).unwrap();
    let plan = engine.resolve(targets, Decisions::Default).unwrap();
    render_plan(&plan)
}

fn first_mismatch(expected: &[String], actual: &[String]) -> Option<usize> {
    let shared = expected.len().min(actual.len());
    for index in 0..shared {
        if expected[index] != actual[index] {
            return Some(index);
        }
    }
    if expected.len() != actual.len() {
        return Some(shared);
    }
    None
}

fn mismatch_window(expected: &[String], actual: &[String], at: usize) -> String {
    let widest = expected.len().max(actual.len());
    let low = at.saturating_sub(3);
    let high = (at + 4).min(widest);
    let mut text = String::new();
    for index in low..high {
        match (expected.get(index), actual.get(index)) {
            (Some(oracle), Some(engine)) if oracle == engine => {
                text.push_str(&format!(" {oracle}\n"));
            }
            (Some(oracle), Some(engine)) => {
                text.push_str(&format!("-{oracle}\n+{engine}\n"));
            }
            (Some(oracle), None) => {
                text.push_str(&format!("-{oracle}\n"));
            }
            (None, Some(engine)) => {
                text.push_str(&format!("+{engine}\n"));
            }
            (None, None) => {}
        }
    }
    text
}

struct CaseOutcome {
    passed: bool,
    report: String,
}

fn run_case(index: usize, total: usize, fixture: &PathBuf, targets: &[String]) -> CaseOutcome {
    let (oracle_raw, oracle_stderr) = run_oracle(fixture, targets);
    let engine_raw = run_engine(targets);
    let oracle = collapse_oracle_duplicates(&oracle_raw);
    let engine = engine_raw;
    let passed = oracle == engine;
    let mut report = format!("case {index}/{total} targets: {}\n", targets.join(" "));
    report.push_str(&format!("oracle lines: {}\n", oracle.len()));
    for line in &oracle {
        report.push_str(&format!("  o {line}\n"));
    }
    if !oracle_stderr.trim().is_empty() {
        report.push_str("oracle stderr:\n");
        for line in oracle_stderr.lines() {
            report.push_str(&format!("  e {line}\n"));
        }
    }
    report.push_str(&format!("engine lines: {}\n", engine.len()));
    for line in &engine {
        report.push_str(&format!("  p {line}\n"));
    }
    if passed {
        report.push_str("result: PASS\n");
    } else {
        report.push_str("result: FAIL\n");
        let at = first_mismatch(&oracle, &engine).unwrap_or(0);
        let oracle_line = oracle.get(at).cloned().unwrap_or("<end>".to_string());
        let engine_line = engine.get(at).cloned().unwrap_or("<end>".to_string());
        report.push_str(&format!("first mismatch at line {at}:\n"));
        report.push_str(&format!("-oracle: {oracle_line}\n"));
        report.push_str(&format!("+engine: {engine_line}\n"));
        report.push_str("context (-oracle +engine):\n");
        report.push_str(&mismatch_window(&oracle, &engine, at));
    }
    CaseOutcome { passed, report }
}

#[test]
#[ignore]
fn differential_fuzz_resolve() {
    let started = Instant::now();
    let fixture = fixture_path();
    std::fs::write(&fixture, FIXTURE).unwrap();
    let statics: Vec<String> = STATIC_TARGETS.iter().map(|name| name.to_string()).collect();
    let dynamics = load_dynamic_targets();
    let mut cases: Vec<Vec<String>> = statics.iter().map(|name| vec![name.clone()]).collect();
    cases.extend(dynamics.iter().map(|name| vec![name.clone()]));
    cases.extend(combo_cases(&statics));
    let total = cases.len();
    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create log directory");
    }
    let mut file = std::fs::File::create(&path).expect("failed to create fuzz log");
    let mut header = format!("fixture: {}\n", fixture.display());
    header.push_str(&format!(
        "corpus: {total} cases ({} static, {} dynamic top-{DYNAMIC_TOP_N}, {} combos)\n",
        statics.len(),
        dynamics.len(),
        PAIR_COUNT + TRIPLE_COUNT + 1
    ));
    println!("{header}");
    file.write_all(header.as_bytes())
        .expect("failed to write fuzz log");
    file.flush().expect("failed to flush fuzz log");
    let mut passed = 0;
    let mut failed = 0;
    for (position, targets) in cases.iter().enumerate() {
        let case_started = Instant::now();
        let outcome = run_case(position + 1, total, &fixture, targets);
        let case_secs = case_started.elapsed().as_secs_f64();
        let total_secs = started.elapsed().as_secs_f64();
        if outcome.passed {
            passed += 1;
        } else {
            failed += 1;
        }
        let status = if outcome.passed { "PASS" } else { "FAIL" };
        println!(
            "{status} case {}/{} ({case_secs:.1}s, {total_secs:.0}s total): {}",
            position + 1,
            total,
            targets.join(" ")
        );
        file.write_all(
            format!(
                "{status} case {}/{} ({case_secs:.1}s, {total_secs:.0}s total): {}\n",
                position + 1,
                total,
                targets.join(" ")
            )
            .as_bytes(),
        )
        .expect("failed to write fuzz log");
        file.write_all(outcome.report.as_bytes())
            .expect("failed to write fuzz log");
        file.write_all(b"---\n").expect("failed to write fuzz log");
        file.flush().expect("failed to flush fuzz log");
    }
    let elapsed = started.elapsed();
    let summary = format!(
        "summary: {passed} passed, {failed} failed, {total} total in {}s\n",
        elapsed.as_secs()
    );
    println!("fuzz summary: {passed} passed, {failed} failed, {total} total");
    file.write_all(summary.as_bytes())
        .expect("failed to write fuzz log");
    file.flush().expect("failed to flush fuzz log");
    println!("fuzz log: {}", path.display());
    assert_eq!(failed, 0, "{failed} mismatches");
}
