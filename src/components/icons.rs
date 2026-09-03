#![allow(dead_code)]

use cosmic::widget::{icon, svg};

macro_rules! icon {
    ($name:ident, $file:literal) => {
        pub fn $name() -> icon::Handle {
            const BYTES: &[u8] =
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $file));
            icon::Handle {
                symbolic: true,
                data: icon::Data::Svg(svg::Handle::from_memory(BYTES)),
            }
        }
    };
}

icon!(history, "history.svg");
icon!(scale, "scale.svg");
icon!(user, "user.svg");
icon!(cpu, "cpu.svg");
icon!(hard_drive, "hard-drive.svg");
icon!(star, "star.svg");
icon!(package_check, "package-check.svg");
icon!(circle_check, "circle-check.svg");
icon!(refresh_cw, "refresh-cw.svg");
icon!(arrow_right, "arrow-right.svg");
icon!(external_link, "external-link.svg");
