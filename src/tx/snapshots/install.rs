use super::{approved, render_outcome, snapshot_settings};
use crate::install::{OfflinePkg, drive_sync, offline_pkg, offline_root};

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

fn run_case(case: &InstallCase) -> String {
    let (_dir, mut handle) = offline_root(&case.packages);
    for ignored in &case.ignore {
        handle.add_ignorepkg(*ignored).unwrap();
    }
    for target in &case.preinstall {
        drive_sync(&mut handle, std::slice::from_ref(target), approved()).unwrap();
    }
    let result = drive_sync(&mut handle, &case.targets, approved());
    render_outcome(&result, &handle)
}

#[test]
fn install_behavior_snapshot() {
    snapshot_settings().bind(|| {
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
