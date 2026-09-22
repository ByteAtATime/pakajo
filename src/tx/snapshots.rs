use crate::install::{OfflinePkg, drive_sync, offline_pkg, offline_root};
use crate::question::source::ExploreDefaults;
use crate::tx::prompt::with_preapproved_proceed;

struct InstallCase {
    name: &'static str,
    packages: Vec<OfflinePkg>,
    preinstall: Vec<&'static str>,
    ignore: Vec<&'static str>,
    targets: Vec<&'static str>,
}

fn cases() -> Vec<InstallCase> {
    let netapp = OfflinePkg {
        name: "netapp",
        depends: &["sdl"],
        ..offline_pkg("netapp")
    };
    let app = OfflinePkg {
        name: "app",
        depends: &["lib"],
        ..offline_pkg("app")
    };
    let gvim = OfflinePkg {
        name: "gvim",
        conflicts: &["vim"],
        ..offline_pkg("gvim")
    };
    vec![
        InstallCase {
            name: "single-package install",
            packages: vec![offline_pkg("sl")],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["sl"],
        },
        InstallCase {
            name: "dependency chain",
            packages: vec![app, offline_pkg("lib")],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["app"],
        },
        InstallCase {
            name: "conflict vim/gvim",
            packages: vec![offline_pkg("vim"), gvim],
            preinstall: vec!["vim"],
            ignore: Vec::new(),
            targets: vec!["gvim"],
        },
        InstallCase {
            name: "provider netapp sdl",
            packages: vec![
                netapp,
                OfflinePkg {
                    name: "sdl-one",
                    provides: &["sdl"],
                    ..offline_pkg("sdl-one")
                },
                OfflinePkg {
                    name: "sdl-two",
                    provides: &["sdl"],
                    ..offline_pkg("sdl-two")
                },
            ],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["netapp"],
        },
        InstallCase {
            name: "ignorepkg skipme",
            packages: vec![offline_pkg("skipme")],
            preinstall: Vec::new(),
            ignore: vec!["skipme"],
            targets: vec!["skipme"],
        },
    ]
}

fn approved() -> Box<dyn crate::question::source::AnswerSource> {
    with_preapproved_proceed(Box::new(ExploreDefaults))
}

fn installed_names(handle: &alpm::Alpm) -> Vec<String> {
    let mut installed: Vec<String> = handle
        .localdb()
        .pkgs()
        .iter()
        .map(|pkg| format!("{} {} {:?}", pkg.name(), pkg.version(), pkg.reason()))
        .collect();
    installed.sort();
    installed
}

fn summary_lines(outcome: &crate::tx::driver::RunOutcome) -> Vec<String> {
    let mut packages: Vec<String> = outcome
        .summary
        .packages
        .iter()
        .map(|pkg| {
            if pkg.is_removal {
                format!(
                    "{} {} (removal)",
                    pkg.name,
                    pkg.old_version.as_deref().unwrap_or("-"),
                )
            } else {
                format!("{} {}", pkg.name, pkg.new_version)
            }
        })
        .collect();
    packages.sort();
    packages
}

fn run_case(case: &InstallCase) -> String {
    let (_dir, mut handle) = offline_root(&case.packages);
    for ignored in &case.ignore {
        handle.add_ignorepkg(*ignored).unwrap();
    }
    for target in &case.preinstall {
        drive_sync(&mut handle, std::slice::from_ref(target), approved()).unwrap();
    }
    let outcome = drive_sync(&mut handle, &case.targets, approved()).unwrap();
    let mut rendered = format!("finish: {:?}\n", outcome.finish);
    rendered.push_str("summary:\n");
    for line in summary_lines(&outcome) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered.push_str("installed:\n");
    for line in installed_names(&handle) {
        rendered.push_str(&format!("  {line}\n"));
    }
    rendered
}

// TODO: ideally this would be an E2E test or something
// but idk how to do that deterministically and lightweight-ly
#[test]
fn install_behavior_snapshot() {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"/tmp/\.tmp[a-zA-Z0-9]+", "<tmp>");
    settings.bind(|| {
        let mut rendered = String::new();
        for (index, case) in cases().iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&format!("== {} ==\n{}", case.name, run_case(case)));
        }
        insta::assert_snapshot!("install_behavior_snapshot", rendered);
    });
}
