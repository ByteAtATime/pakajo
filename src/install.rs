use anyhow::{Context, anyhow};

pub fn run_install(name: &str) -> anyhow::Result<()> {
    let config = pacmanconf::Config::new().context("failed to read pacman config")?;
    let mut handle = crate::init_alpm(&config)?;
    let result = run_transaction(&mut handle, name);
    let _ = handle.trans_release();
    result
}

fn run_transaction(handle: &mut alpm::Alpm, name: &str) -> anyhow::Result<()> {
    handle
        .trans_init(alpm::TransFlag::NONE)
        .context("failed to initialize transaction")?;
    let pkg = crate::find_pkg(handle, name)
        .ok_or_else(|| anyhow!("package '{name}' not found in any repository"))?;
    handle
        .trans_add_pkg(pkg)
        .map_err(alpm::Error::from)
        .context("failed to queue package for installation")?;
    handle
        .trans_prepare()
        .map_err(alpm::Error::from)
        .context("failed to prepare transaction")?;
    handle
        .trans_commit()
        .context("failed to commit transaction")?;
    Ok(())
}
