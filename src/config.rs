use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

const TEMPLATE: &str = include_str!("config/template.toml");

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub cli: CliConfig,
    pub aur: AurConfig,
    pub build: BuildConfig,
    pub gui: GuiConfig,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    #[serde(default)]
    pub pager: String,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AurConfig {
    pub skip_review: bool,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    #[serde(default)]
    pub onboarded: bool,
}

fn default_keep_cache() -> bool {
    true
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    #[serde(default = "default_keep_cache")]
    pub keep_cache: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            keep_cache: default_keep_cache(),
        }
    }
}

fn allowed_keys(section: &str) -> Option<&'static [&'static str]> {
    match section {
        "cli" => Some(&["pager"]),
        "aur" => Some(&["skip_review"]),
        "build" => Some(&["keep_cache"]),
        "gui" => Some(&["onboarded"]),
        _ => None,
    }
}

fn parse_config(text: &str) -> anyhow::Result<(Config, Vec<String>)> {
    let table: toml::Table = toml::from_str(text)?;
    let mut warnings = Vec::new();
    for (key, value) in &table {
        let Some(allowed) = allowed_keys(key) else {
            warnings.push(format!("unknown section '{key}'"));
            continue;
        };
        if let Some(inner) = value.as_table() {
            for inner_key in inner.keys() {
                if !allowed.contains(&inner_key.as_str()) {
                    warnings.push(format!("unknown key '{inner_key}' in [{key}]"));
                }
            }
        }
    }
    let config: Config = toml::from_str(text)?;
    Ok((config, warnings))
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    Ok(crate::utils::config_base()?
        .join("pakajo")
        .join("config.toml"))
}

fn load_or_create_at(path: &Path) -> anyhow::Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let (config, warnings) = parse_config(&text)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            for w in &warnings {
                eprintln!("warning: config.toml: {w}");
            }
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!(
                    "warning: failed to create config directory {}: {e:#}, using defaults",
                    parent.display()
                );
                return Ok(Config::default());
            }
            match std::fs::write(path, TEMPLATE.as_bytes()) {
                Ok(()) => {
                    eprintln!("[pakajo] created config template at {}", path.display());
                    Ok(Config::default())
                }
                Err(e) => {
                    eprintln!(
                        "warning: failed to write config template {}: {e:#}, using defaults",
                        path.display()
                    );
                    Ok(Config::default())
                }
            }
        }
        Err(e) => {
            eprintln!(
                "warning: failed to read {}: {e:#}, using defaults",
                path.display()
            );
            Ok(Config::default())
        }
    }
}

pub fn load_or_create() -> anyhow::Result<Config> {
    let path = match config_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("warning: cannot determine config location: {e:#}, using defaults");
            return Ok(Config::default());
        }
    };
    load_or_create_at(&path)
}

pub fn complete_onboarding_at(path: &Path, keep_cache: Option<bool>) -> anyhow::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    parse_config(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    doc["gui"]["onboarded"] = toml_edit::value(true);
    if let Some(v) = keep_cache {
        doc["build"]["keep_cache"] = toml_edit::value(v);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    std::fs::write(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn complete_onboarding(keep_cache: Option<bool>) -> anyhow::Result<()> {
    let path = config_path().with_context(|| "cannot determine config location")?;
    complete_onboarding_at(&path, keep_cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn empty_text_yields_defaults_without_warnings() {
        let (config, warnings) = parse_config("").expect("empty parses");
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn cli_pager_parses() {
        let (config, warnings) = parse_config("[cli]\npager = \"most\"\n").expect("parses");
        assert_eq!(config.cli.pager, "most");
        assert!(warnings.is_empty());
    }

    #[test]
    fn pager_type_error_mentions_location() {
        let err = match parse_config("[cli]\npager = 3\n") {
            Ok(_) => panic!("type error accepted"),
            Err(err) => err,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("line") || msg.contains("column") || msg.contains("2"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn typo_key_warns_as_unknown_key() {
        let (config, warnings) = parse_config("[cli]\npaager = \"most\"\n").expect("typo parses");
        assert_eq!(config, Config::default());
        assert!(warnings.iter().any(|w| w.contains("paager")));
    }

    #[test]
    fn unknown_top_level_section_warns() {
        let (_, warnings) = parse_config("[mystery]\nfoo = 1\n").expect("parses");
        assert!(warnings.iter().any(|w| w.contains("mystery")));
    }

    #[test]
    fn aur_skip_review_parses() {
        let (config, warnings) = parse_config("[aur]\nskip_review = true\n").expect("parses");
        assert!(config.aur.skip_review);
        assert_eq!(config.cli, CliConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn aur_typo_key_warns() {
        let (config, warnings) = parse_config("[aur]\nskip_reviw = true\n").expect("typo parses");
        assert_eq!(config, Config::default());
        assert!(warnings.iter().any(|w| w.contains("skip_reviw")));
    }

    #[test]
    fn build_keep_cache_false_parses() {
        let (config, warnings) = parse_config("[build]\nkeep_cache = false\n").expect("parses");
        assert!(!config.build.keep_cache);
        assert!(warnings.is_empty());
    }

    #[test]
    fn default_keep_cache_is_true() {
        assert!(Config::default().build.keep_cache);
    }

    #[test]
    fn typo_build_key_warns() {
        let (config, warnings) = parse_config("[build]\nkeep_cach = false\n").expect("typo parses");
        assert!(config.build.keep_cache);
        assert!(warnings.iter().any(|w| w.contains("keep_cach")));
    }

    #[test]
    fn empty_build_section_defaults_true() {
        let (config, warnings) = parse_config("[build]\n").expect("parses");
        assert!(config.build.keep_cache);
        assert!(warnings.is_empty());
    }

    #[test]
    fn missing_file_creates_template_with_defaults() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        let path = root.0.join("pakajo").join("config.toml");
        let config = load_or_create_at(&path).expect("creates");
        assert_eq!(config, Config::default());
        let bytes = std::fs::read(&path).expect("template written");
        assert_eq!(bytes, TEMPLATE.as_bytes());
    }

    #[test]
    fn corrupt_file_returns_error() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-corrupt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        std::fs::write(&path, "[cli]\npager = 3\n").expect("write");
        assert!(load_or_create_at(&path).is_err());
    }

    #[test]
    fn valid_file_parses_without_rewrite() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-valid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        let body = "[cli]\npager = \"most\"\n";
        std::fs::write(&path, body).expect("write");
        let config = load_or_create_at(&path).expect("parses");
        assert_eq!(config.cli.pager, "most");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(after, body);
    }

    #[test]
    fn gui_onboarded_parses_with_default_false() {
        let (config, warnings) = parse_config("[gui]\nonboarded = true\n").expect("parses");
        assert!(config.gui.onboarded);
        assert!(warnings.is_empty());
        let (config, warnings) = parse_config("").expect("empty parses");
        assert!(!config.gui.onboarded);
        assert!(warnings.is_empty());
    }

    #[test]
    fn complete_onboarding_preserves_comments_and_unknown_sections() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-onboard-preserve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        let body = "# keep this comment\n[aur]\nskip_review = true\n[mystery]\nfoo = 1\n";
        std::fs::write(&path, body).expect("write");
        complete_onboarding_at(&path, None).expect("completes");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(after.contains("onboarded = true"));
        assert!(after.contains("# keep this comment"));
        assert!(after.contains("mystery"));
        let (config, _) = parse_config(&after).expect("reparses");
        assert!(config.gui.onboarded);
        assert!(config.aur.skip_review);
    }

    #[test]
    fn complete_onboarding_writes_keep_cache() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-onboard-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        std::fs::write(&path, "[build]\nkeep_cache = true\n").expect("write");
        complete_onboarding_at(&path, Some(false)).expect("completes");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(after.contains("keep_cache = false"));
        assert!(after.contains("onboarded = true"));
    }

    #[test]
    fn complete_onboarding_without_keep_cache_leaves_build_untouched() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-onboard-nocache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        std::fs::write(&path, "[build]\nkeep_cache = true\n").expect("write");
        complete_onboarding_at(&path, None).expect("completes");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(after.contains("keep_cache = true"));
    }

    #[test]
    fn complete_onboarding_missing_file_writes_template() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-onboard-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        let path = root.0.join("pakajo").join("config.toml");
        complete_onboarding_at(&path, None).expect("completes");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(after.contains("# Pager used when reviewing PKGBUILDs."));
        assert!(after.contains("onboarded = true"));
    }

    #[test]
    fn complete_onboarding_corrupt_file_errors_without_modifying() {
        let root = TempDir(std::env::temp_dir().join(format!(
            "pakajo-config-onboard-corrupt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )));
        std::fs::create_dir_all(&root.0).expect("mkdir");
        let path = root.0.join("config.toml");
        let body = "[cli]\npager = 3\n";
        std::fs::write(&path, body).expect("write");
        assert!(complete_onboarding_at(&path, None).is_err());
        let after = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(after, body);
    }
}
