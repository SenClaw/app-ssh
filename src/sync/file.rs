//! Private, atomic writes for the app's data files, and the one-time backup
//! and permission pass that runs when sync first sees an existing data dir.
//!
//! The files hold plaintext passwords and private keys, so they are created
//! owner-only, and a write goes to a sibling file that is then renamed over the
//! target: a crash leaves the old file or the new one, never half of one.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The collections' files (and settings, which holds no secrets but lives
/// beside them and gets the same mode).
pub const DATA_FILES: [&str; 5] =
    ["hosts.json", "keychain.json", "snippets.json", "port_forwarding.json", "settings.json"];

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

/// Owner-only read/write, where the platform has such a thing.
pub fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Replace `path` with `data`, owner-only, all or nothing as far as a crash of
/// this process goes. It does not fsync: the files were never fsynced before
/// either, and sync writes one per record.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    {
        let mut f = create_private(&tmp)?;
        // A temp file left by a crash may predate this code and be world-readable.
        restrict(&tmp);
        f.write_all(data)?;
    }
    fs::rename(&tmp, path)
}

/// Last-modified time in epoch milliseconds.
pub fn mtime_ms(path: &Path) -> Option<u64> {
    let t = fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Copy every existing data file to `<name>.pre-sync.bak` (owner-only), once.
/// Returns the names written.
pub fn backup_once(dir: &Path) -> Vec<String> {
    let mut done = Vec::new();
    for name in DATA_FILES {
        let src = dir.join(name);
        let bak = dir.join(format!("{name}.pre-sync.bak"));
        if !src.exists() || bak.exists() {
            continue;
        }
        if let Ok(bytes) = fs::read(&src) {
            if write_private(&bak, &bytes).is_ok() {
                done.push(format!("{name}.pre-sync.bak"));
            }
        }
    }
    done
}

/// Make the directory and the files in it owner-only.
pub fn harden(dir: &Path) {
    restrict(dir);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_file() {
            restrict(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::testutil::scratch;

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_write_is_owner_only_and_replaces_atomically() {
        let dir = scratch("write");
        let p = dir.join("hosts.json");
        write_private(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(mode(&p), 0o600);
        assert!(!tmp_path(&p).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn harden_tightens_a_world_readable_file_and_the_folder() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("harden");
        let p = dir.join("hosts.json");
        fs::write(&p, "{}").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        harden(&dir);
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(&dir), 0o700);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backup_is_made_once_and_never_overwritten() {
        let dir = scratch("backup");
        fs::write(dir.join("hosts.json"), "original").unwrap();
        assert_eq!(backup_once(&dir), vec!["hosts.json.pre-sync.bak".to_string()]);
        fs::write(dir.join("hosts.json"), "edited").unwrap();
        assert!(backup_once(&dir).is_empty());
        assert_eq!(fs::read_to_string(dir.join("hosts.json.pre-sync.bak")).unwrap(), "original");
        fs::remove_dir_all(dir).unwrap();
    }
}
