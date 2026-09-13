use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

pub struct ApprovalsFile {
    path: PathBuf,
}

impl ApprovalsFile {
    pub fn write(payload: &[u8]) -> anyhow::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pakajo-{nanos}.json"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(payload)?;
        Ok(ApprovalsFile { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ApprovalsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_file_has_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let payload = br#"{"approved_conflicts":[]}"#;
        let file = ApprovalsFile::write(payload).expect("write approvals file");
        let mode = std::fs::metadata(file.path())
            .expect("stat approvals file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn approvals_file_deleted_after_normal_exit() {
        let path = {
            let file = ApprovalsFile::write(br#"{"approved_conflicts":[]}"#).expect("write file");
            let path = file.path().to_path_buf();
            assert!(path.exists());
            drop(file);
            path
        };
        assert!(!path.exists());
    }
}
