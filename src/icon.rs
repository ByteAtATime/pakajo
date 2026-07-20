use anyhow::{Result, anyhow};
use gpui::{App, AssetSource, IntoElement, RenderOnce, SharedString, Window};
use gpui_component::{Icon, IconNamed};
use rust_embed::RustEmbed;

#[derive(IntoElement)]
pub enum PakajoIcon {
    #[allow(dead_code)]
    History,
    Scale,
    User,
    Cpu,
    HardDrive,
    Star,
    PackageCheck,
}

impl IconNamed for PakajoIcon {
    fn path(self) -> SharedString {
        match self {
            PakajoIcon::History => "icons/history.svg",
            PakajoIcon::Scale => "icons/scale.svg",
            PakajoIcon::User => "icons/user.svg",
            PakajoIcon::Cpu => "icons/cpu.svg",
            PakajoIcon::HardDrive => "icons/hard-drive.svg",
            PakajoIcon::Star => "icons/star.svg",
            PakajoIcon::PackageCheck => "icons/package-check.svg",
        }
        .into()
    }
}

impl RenderOnce for PakajoIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::new(self)
    }
}

#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

pub struct CombinedAssets;

impl AssetSource for CombinedAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Ok(Some(data)) = Assets.load(path) {
            return Ok(Some(data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out = Assets.list(path)?;
        out.extend(gpui_component_assets::Assets.list(path)?);
        Ok(out)
    }
}
