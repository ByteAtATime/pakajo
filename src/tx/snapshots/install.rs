use super::{approved, render_outcome, snapshot_settings};
use crate::tx::fixtures::{Pkg, drive_sync, fixture};

struct InstallCase {
    name: &'static str,
    packages: Vec<Pkg>,
    preinstall: Vec<&'static str>,
    ignore: Vec<&'static str>,
    targets: Vec<&'static str>,
}

fn cases() -> Vec<InstallCase> {
    let netapp = Pkg {
        depends: vec!["sdl"],
        ..Pkg::plain("netapp")
    };
    let app = Pkg {
        depends: vec!["lib"],
        ..Pkg::plain("app")
    };
    let gvim = Pkg {
        conflicts: vec!["vim"],
        ..Pkg::plain("gvim")
    };
    vec![
        InstallCase {
            name: "single-package install",
            packages: vec![Pkg::plain("sl")],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["sl"],
        },
        InstallCase {
            name: "dependency chain",
            packages: vec![app, Pkg::plain("lib")],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["app"],
        },
        InstallCase {
            name: "conflict vim/gvim",
            packages: vec![Pkg::plain("vim"), gvim],
            preinstall: vec!["vim"],
            ignore: Vec::new(),
            targets: vec!["gvim"],
        },
        InstallCase {
            name: "provider netapp sdl",
            packages: vec![
                netapp,
                Pkg {
                    provides: vec!["sdl"],
                    ..Pkg::plain("sdl-one")
                },
                Pkg {
                    provides: vec!["sdl"],
                    ..Pkg::plain("sdl-two")
                },
            ],
            preinstall: Vec::new(),
            ignore: Vec::new(),
            targets: vec!["netapp"],
        },
        InstallCase {
            name: "ignorepkg skipme",
            packages: vec![Pkg::plain("skipme")],
            preinstall: Vec::new(),
            ignore: vec!["skipme"],
            targets: vec!["skipme"],
        },
    ]
}

fn run_case(case: &InstallCase) -> String {
    let (_dir, mut handle) = fixture(&case.packages);
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
