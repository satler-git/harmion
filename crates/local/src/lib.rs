//! Native owner-only file access shared by the daemon and local SDK discovery.
//! The initial implementation supports Unix; unsupported hosts fail closed.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct ProtectedDir {
    root: PathBuf,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(unix)]
fn check(metadata: &fs::Metadata, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(invalid("expected an owner-only directory or regular file"));
    }
    Ok(())
}
#[cfg(not(unix))]
fn check(_: &fs::Metadata, _: bool) -> io::Result<()> {
    Err(invalid(
        "owner-only host storage is not implemented on this OS",
    ))
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options
}

impl ProtectedDir {
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        #[cfg(not(unix))]
        return Err(invalid(
            "owner-only host storage is not implemented on this OS",
        ));
        builder.create(path.as_ref())?;
        Self::open(path)
    }
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        check(&fs::symlink_metadata(path.as_ref())?, true)?;
        Ok(Self {
            root: fs::canonicalize(path)?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    fn file_path(&self, name: &str) -> io::Result<PathBuf> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(invalid("invalid local filename"));
        }
        check(&fs::symlink_metadata(&self.root)?, true)?;
        Ok(self.root.join(name))
    }
    pub fn verify_file(&self, name: &str) -> io::Result<()> {
        let file = options().read(true).open(self.file_path(name)?)?;
        check(&file.metadata()?, false)
    }
    pub fn read(&self, name: &str, max: usize) -> io::Result<Vec<u8>> {
        let file = options().read(true).open(self.file_path(name)?)?;
        check(&file.metadata()?, false)?;
        let mut bytes = Vec::new();
        file.take(max as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "local file exceeds limit",
            ));
        }
        Ok(bytes)
    }
    pub fn create_file(&self, name: &str) -> io::Result<File> {
        let file = options()
            .write(true)
            .create_new(true)
            .open(self.file_path(name)?)?;
        check(&file.metadata()?, false)?;
        Ok(file)
    }
    pub fn write_new(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.create_file(name)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        File::open(&self.root)?.sync_all()
    }
    pub fn replace(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let target = self.file_path(name)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) => check(&metadata, false)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        let suffix = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let temp = format!(".{name}.{suffix}.tmp");
        self.write_new(&temp, bytes)?;
        fs::rename(self.root.join(temp), target)?;
        File::open(&self.root)?.sync_all()
    }
    /// Keep this file alive for the entire daemon lifetime.
    pub fn lock(&self) -> io::Result<File> {
        let path = self.file_path("daemon.lock")?;
        let file = match options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                options().read(true).write(true).open(path)?
            }
            Err(e) => return Err(e),
        };
        check(&file.metadata()?, false)?;
        file.try_lock()
            .map_err(|_| io::Error::other("node is already running or cannot be locked"))?;
        Ok(file)
    }
    /// Lock an existing daemon.lock without creating it. Returns None for a
    /// static backup/snapshot that never had a live daemon lock.
    pub fn try_lock_existing(&self) -> io::Result<Option<File>> {
        let path = self.file_path("daemon.lock")?;
        let file = match options().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        check(&file.metadata()?, false)?;
        file.try_lock()
            .map_err(|_| io::Error::other("node is already running or cannot be locked"))?;
        Ok(Some(file))
    }
}
