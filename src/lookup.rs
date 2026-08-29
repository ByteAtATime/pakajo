use crate::{aur::AurClient, package::Package, pacman::find_pkg};
use alpm::Alpm;

pub fn lookup(alpm: &Alpm, aur: &AurClient, name: &str) -> Option<Package> {
    if let Some(pkg) = find_pkg(alpm, name) {
        return Some(Package::from(pkg));
    }
    let info = aur.info(name).ok()??;
    Some(Package::from(info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{aur::AurClient, package::PackageSource, pacman::init_alpm};

    #[test]
    #[ignore]
    fn live_lookup() {
        let config = pacmanconf::Config::new().expect("read pacman config");
        let alpm = init_alpm(&config).expect("init alpm");
        let aur = AurClient::new();

        let sl = lookup(&alpm, &aur, "sl").expect("sl should resolve from repo");
        assert_eq!(sl.source, PackageSource::Repo);

        let chrome =
            lookup(&alpm, &aur, "google-chrome").expect("google-chrome should resolve via AUR");
        assert_eq!(chrome.source, PackageSource::Aur);
        assert_eq!(chrome.repo.as_deref(), Some("aur"));

        assert!(lookup(&alpm, &aur, "zzz-not-real").is_none());
    }
}
