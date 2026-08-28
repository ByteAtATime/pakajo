#![allow(dead_code)]

use cosmic::Theme;
use cosmic::widget::{Svg, svg};

macro_rules! icon {
    ($name:ident, $file:literal) => {
        pub fn $name<'a>() -> Svg<'a, Theme> {
            const BYTES: &[u8] =
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $file));
            svg(svg::Handle::from_memory(BYTES)).class(cosmic::theme::Svg::Custom(
                std::rc::Rc::new(|theme: &cosmic::Theme| svg::Style {
                    color: Some(theme.cosmic().background(false).on.into()),
                }),
            ))
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
