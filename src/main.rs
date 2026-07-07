mod package;

use alpm::Alpm;
use gpui::*;
use gpui_component::*;

use crate::package::Package;

struct PackageListing {
    pkg: Package,
}

impl Render for PackageListing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .bg(rgb(0xcccccc))
            .gap_2()
            .p_4()
            .size_full()
            .child(self.pkg.name.clone())
            .child(self.pkg.description.clone().unwrap_or_default())
            .child(self.pkg.version.clone())
    }
}

struct PakajoRoot {
    alpm_handle: Alpm,
}

impl Render for PakajoRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let db = self.alpm_handle.localdb();
        let packages = db.pkgs();

        div()
            .v_flex()
            .gap_4()
            .p_4()
            .size_full()
            .items_center()
            .justify_center()
            .children(packages.into_iter().map(|pkg| {
                let package_listing = PackageListing { pkg: pkg.into() };
                cx.new(|_| package_listing)
            }))
    }
}

fn main() {
    let handle = Alpm::new("/", "/var/lib/pacman").expect("Couldn't initialize alpm");

    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);

    app.run(move |cx| {
        gpui_component::init(cx);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| PakajoRoot {
                    alpm_handle: handle,
                });
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}
