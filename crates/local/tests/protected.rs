#![cfg(unix)]
use harmion_local::ProtectedDir;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        Self(std::env::temp_dir().join(format!("harmion-protected-{:x?}", bytes)))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn owner_only_files_bounded_reads_and_atomic_replacement() {
    let temp = Temp::new();
    let directory = ProtectedDir::create(&temp.0).unwrap();
    assert_eq!(
        fs::metadata(&temp.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(ProtectedDir::create(&temp.0).is_err());
    directory.write_new("secret", b"first").unwrap();
    assert_eq!(
        fs::metadata(temp.0.join("secret"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(directory.write_new("secret", b"overwrite").is_err());
    assert!(directory.read("secret", 4).is_err());
    directory.replace("secret", b"second").unwrap();
    assert_eq!(directory.read("secret", 6).unwrap(), b"second");
    assert!(directory.read("../secret", 100).is_err());
    assert!(directory.write_new("/tmp/outside", b"no").is_err());
    fs::set_permissions(temp.0.join("secret"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(directory.read("secret", 100).is_err());
    assert!(directory.replace("secret", b"no").is_err());
    fs::set_permissions(&temp.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ProtectedDir::open(&temp.0).is_err());
    assert!(directory.write_new("other", b"no").is_err());
}

#[test]
fn symlinks_hardlinks_and_multiple_daemon_locks_are_rejected() {
    let temp = Temp::new();
    let directory = ProtectedDir::create(&temp.0).unwrap();
    directory.write_new("secret", b"private").unwrap();
    symlink(temp.0.join("secret"), temp.0.join("link")).unwrap();
    assert!(directory.read("link", 100).is_err());
    assert!(directory.replace("link", b"no").is_err());
    assert_eq!(directory.read("secret", 100).unwrap(), b"private");
    fs::hard_link(temp.0.join("secret"), temp.0.join("hardlink")).unwrap();
    assert!(directory.read("secret", 100).is_err());
    let first = directory.lock().unwrap();
    assert!(directory.lock().is_err());
    drop(first);
    assert!(directory.lock().is_ok());
    symlink(&temp.0, temp.0.join("self")).unwrap();
    assert!(ProtectedDir::open(temp.0.join("self")).is_err());
}
