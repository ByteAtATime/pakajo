use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MISSING_SECS: i64 = i64::MIN;

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    mtime_secs: i64,
    mtime_nanos: u32,
    size: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    key: Vec<FileStamp>,
    installed: Vec<String>,
    groups: Vec<(String, String)>,
}

pub struct AlpmSnapshot {
    pub installed: Vec<String>,
    pub groups: Vec<(String, String)>,
}

fn snapshot_path() -> anyhow::Result<PathBuf> {
    Ok(crate::build::cache_root()?.join("alpm-snapshot.bin"))
}

fn stamp(path: &Path) -> FileStamp {
    let Ok(md) = std::fs::metadata(path) else {
        return FileStamp {
            path: path.to_path_buf(),
            mtime_secs: MISSING_SECS,
            mtime_nanos: 0,
            size: 0,
        };
    };
    let (mtime_secs, mtime_nanos) = match md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    {
        Some(d) => (d.as_secs() as i64, d.subsec_nanos()),
        None => (MISSING_SECS, 0),
    };
    FileStamp {
        path: path.to_path_buf(),
        mtime_secs,
        mtime_nanos,
        size: md.len(),
    }
}

fn collect_includes(conf: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(conf) else {
        return;
    };
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("include") {
            continue;
        }
        let target = value.trim();
        let path = PathBuf::from(target);
        out.push(path.clone());
        if !target.contains('*') && !target.contains('?') {
            collect_includes(&path, out, depth + 1);
        }
    }
}

fn sync_db_stamps(db_path: &str) -> Vec<FileStamp> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(Path::new(db_path).join("sync"))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "db"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths.iter().map(|p| stamp(p)).collect()
}

fn freshness_key(conf: &Path, db_path: &str) -> Vec<FileStamp> {
    let mut includes = vec![conf.to_path_buf()];
    collect_includes(conf, &mut includes, 0);
    let mut key: Vec<FileStamp> = includes.iter().map(|p| stamp(p)).collect();
    key.extend(sync_db_stamps(db_path));
    key.push(stamp(&Path::new(db_path).join("local")));
    key
}

fn load(path: &Path, key: &[FileStamp]) -> Option<AlpmSnapshot> {
    let bytes = std::fs::read(path).ok()?;
    let (file, _) =
        bincode::serde::decode_from_slice::<SnapshotFile, _>(&bytes, bincode::config::standard())
            .ok()?;
    if file.key != key {
        return None;
    }
    Some(AlpmSnapshot {
        installed: file.installed,
        groups: file.groups,
    })
}

fn save(path: &Path, file: &SnapshotFile) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("alpm-snapshot.bin.tmp{}", std::process::id()));
    let bytes = bincode::serde::encode_to_vec(file, bincode::config::standard())?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn get(config: &pacmanconf::Config) -> anyhow::Result<AlpmSnapshot> {
    let path = snapshot_path()?;
    let key = freshness_key(Path::new("/etc/pacman.conf"), &config.db_path);
    if let Some(snapshot) = load(&path, &key) {
        return Ok(snapshot);
    }
    let handle = crate::pacman::init_alpm(config)?;
    let installed = crate::package::installed_names(&handle)
        .into_iter()
        .collect::<Vec<String>>();
    let groups = crate::pacman::collect_group_index(&handle);
    let file = SnapshotFile {
        key,
        installed: installed.clone(),
        groups: groups.clone(),
    };
    let _ = save(&path, &file);
    Ok(AlpmSnapshot { installed, groups })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write_conf(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("pacman.conf");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn fake_db_root(dir: &Path) -> String {
        let root = dir.join("db");
        std::fs::create_dir_all(root.join("sync")).unwrap();
        std::fs::create_dir_all(root.join("local")).unwrap();
        std::fs::write(root.join("sync/core.db"), b"coredb").unwrap();
        root.to_str().unwrap().to_string()
    }

    fn sample_file(key: Vec<FileStamp>) -> SnapshotFile {
        SnapshotFile {
            key,
            installed: vec!["vim".to_string(), "sl".to_string()],
            groups: vec![
                ("base-devel".to_string(), "core".to_string()),
                ("gnome".to_string(), "extra".to_string()),
            ],
        }
    }

    #[test]
    fn save_and_load_preserves_field_order() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "[options]\n");
        let db = fake_db_root(dir.path());
        let key = freshness_key(&conf, &db);
        let path = dir.path().join("alpm-snapshot.bin");
        save(&path, &sample_file(key.clone())).unwrap();
        let snap = load(&path, &key).unwrap();
        assert_eq!(snap.installed, vec!["vim".to_string(), "sl".to_string()]);
        assert_eq!(snap.groups.len(), 2);
        assert_eq!(snap.groups[0].0, "base-devel");
        assert_eq!(snap.groups[0].1, "core");
        assert_eq!(snap.groups[1].0, "gnome");
        assert_eq!(snap.groups[1].1, "extra");
    }

    #[test]
    fn load_misses_when_local_db_dir_changes() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "[options]\n");
        let db = fake_db_root(dir.path());
        let key = freshness_key(&conf, &db);
        let path = dir.path().join("alpm-snapshot.bin");
        save(&path, &sample_file(key)).unwrap();
        let local = dir.path().join("db/local");
        let marker = local.join("pkg-1.0-1");
        std::fs::create_dir_all(&marker).unwrap();
        std::fs::remove_dir(&marker).unwrap();
        assert!(load(&path, &freshness_key(&conf, &db)).is_none());
    }

    #[test]
    fn load_misses_when_sync_db_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "[options]\n");
        let db = fake_db_root(dir.path());
        let key = freshness_key(&conf, &db);
        let path = dir.path().join("alpm-snapshot.bin");
        save(&path, &sample_file(key)).unwrap();
        let core = dir.path().join("db/sync/core.db");
        let later = std::time::SystemTime::now() + Duration::from_secs(3600);
        std::fs::File::open(&core)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(load(&path, &freshness_key(&conf, &db)).is_none());
    }

    #[test]
    fn load_misses_when_sync_db_size_changes() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "[options]\n");
        let db = fake_db_root(dir.path());
        let key = freshness_key(&conf, &db);
        let path = dir.path().join("alpm-snapshot.bin");
        save(&path, &sample_file(key)).unwrap();
        let core = dir.path().join("db/sync/core.db");
        std::fs::write(core, b"coredb-longer-v2").unwrap();
        assert!(load(&path, &freshness_key(&conf, &db)).is_none());
    }

    #[test]
    fn garbage_file_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alpm-snapshot.bin");
        std::fs::write(&path, b"definitely not bincode").unwrap();
        assert!(load(&path, &[]).is_none());
    }

    #[test]
    fn missing_include_invalidates_key() {
        let dir = tempfile::tempdir().unwrap();
        let include = dir.path().join("mirrorlist");
        std::fs::write(&include, b"[core]\nServer = http://mirror\n").unwrap();
        let conf = write_conf(
            dir.path(),
            &format!("[options]\nInclude = {}\n", include.display()),
        );
        let db = fake_db_root(dir.path());
        let key = freshness_key(&conf, &db);
        let path = dir.path().join("alpm-snapshot.bin");
        save(&path, &sample_file(key.clone())).unwrap();
        assert!(load(&path, &key).is_some());
        std::fs::remove_file(&include).unwrap();
        assert!(load(&path, &freshness_key(&conf, &db)).is_none());
    }
}
