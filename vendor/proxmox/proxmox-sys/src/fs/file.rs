use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
#[cfg(feature = "timer")]
use std::time::Duration;

use anyhow::{Context as _, Error, bail, format_err};
use nix::NixPath;
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::stat;
use nix::unistd;
use serde_json::Value;

use crate::error::SysError;

#[cfg(feature = "timer")]
use crate::error::SysResult;
#[cfg(all(feature = "timer", target_os = "linux"))]
use crate::linux::timer;

use crate::fs::CreateOptions;

/// Read the entire contents of a file into a bytes vector
///
/// This basically call ``std::fs::read``, but provides more elaborate
/// error messages including the path.
pub fn file_get_contents<P: AsRef<Path>>(path: P) -> Result<Vec<u8>, Error> {
    let path = path.as_ref();

    std::fs::read(path).map_err(|err| format_err!("unable to read {:?} - {}", path, err))
}

/// Read the entire contents of a file into a bytes vector if the file exists
///
/// Same as file_get_contents(), but returns 'Ok(None)' instead of
/// 'Err' if the file does not exist.
pub fn file_get_optional_contents<P: AsRef<Path>>(path: P) -> Result<Option<Vec<u8>>, Error> {
    let path = path.as_ref();

    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                bail!("unable to read '{:?}' - {}", path, err);
            }
        }
    }
}

/// Read the entire contents of a file into a String
///
/// This basically call ``std::fs::read_to_string``, but provides more elaborate
/// error messages including the path.
pub fn file_read_string<P: AsRef<Path>>(path: P) -> Result<String, Error> {
    let path = path.as_ref();

    std::fs::read_to_string(path).map_err(|err| format_err!("unable to read {:?} - {}", path, err))
}

/// Read the entire contents of a file into a String if the file exists
///
/// Same as file_read_string(), but returns 'Ok(None)' instead of
/// 'Err' if the file does not exist.
pub fn file_read_optional_string<P: AsRef<Path>>(path: P) -> Result<Option<String>, Error> {
    let path = path.as_ref();

    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                bail!("unable to read '{:?}' - {}", path, err);
            }
        }
    }
}

/// Read .json file into a ``Value``
///
/// The optional ``default`` is used when the file does not exist.
pub fn file_get_json<P: AsRef<Path>>(path: P, default: Option<Value>) -> Result<Value, Error> {
    let path = path.as_ref();

    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .with_context(|| format!("unable to parse json from {path:?}")),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                if let Some(v) = default {
                    return Ok(v);
                }
            }
            bail!("unable to read json {:?} - {}", path, err);
        }
    }
}

/// Read the first line of a file as String in std IO error context
pub(crate) fn read_firstline<P: AsRef<Path>>(path: P) -> Result<String, std::io::Error> {
    let file = std::fs::File::open(path)?;

    let mut reader = BufReader::new(file);
    let mut line = String::new();

    let _ = reader.read_line(&mut line)?;

    Ok(line)
}

/// Read the first line of a file as String
pub fn file_read_firstline<P: AsRef<Path>>(path: P) -> Result<String, Error> {
    let path = path.as_ref();
    read_firstline(path).map_err(|err| format_err!("unable to read {path:?} - {err}"))
}

#[inline]
/// Creates a tmpfile like [`nix::unistd::mkstemp`], but with [`OFlag`] set.
///
/// Note that some flags are masked out since they can produce an error, see mkostemp(2) for details.
// code is mostly copied from nix mkstemp
fn mkostemp<P: ?Sized + NixPath>(
    template: &P,
    oflag: OFlag,
) -> nix::Result<(std::os::fd::RawFd, PathBuf)> {
    use std::os::unix::ffi::OsStringExt;
    let mut path = template.with_nix_path(|path| path.to_bytes_with_nul().to_owned())?;
    let p = path.as_mut_ptr().cast();

    let flags = OFlag::intersection(OFlag::O_APPEND | OFlag::O_CLOEXEC | OFlag::O_SYNC, oflag);

    #[cfg(target_os = "linux")]
    let fd = unsafe { libc::mkostemp(p, flags.bits()) };
    // part of libSystem, but the libc crate has no binding for it on macOS
    #[cfg(target_os = "macos")]
    let fd = {
        unsafe extern "C" {
            fn mkostemp(template: *mut libc::c_char, flags: libc::c_int) -> libc::c_int;
        }
        unsafe { mkostemp(p, flags.bits()) }
    };
    let last = path.pop(); // drop the trailing nul
    debug_assert!(last == Some(b'\0'));
    let pathname = std::ffi::OsString::from_vec(path);
    Errno::result(fd)?;
    Ok((fd, PathBuf::from(pathname)))
}

/// Takes a Path and CreateOptions, creates a tmpfile from it and returns
/// a RawFd and PathBuf for it
pub fn make_tmp_file<P: AsRef<Path>>(
    path: P,
    options: CreateOptions,
) -> Result<(File, PathBuf), Error> {
    let path = path.as_ref();

    // use mkstemp here, because it works with different processes, threads, even tokio tasks
    let mut template = path.to_owned();
    template.add_extension("tmp_XXXXXX");
    let (mut file, tmp_path) = match mkostemp(&template, OFlag::O_CLOEXEC) {
        Ok((fd, path)) => (unsafe { File::from_raw_fd(fd) }, path),
        Err(err) => bail!("mkstemp {:?} failed: {}", template, err),
    };

    match options.apply_to(&mut file, &tmp_path) {
        Ok(()) => Ok((file, tmp_path)),
        Err(err) => {
            let _ = unistd::unlink(&tmp_path);
            Err(err)
        }
    }
}

/// Atomically replace a file.
///
/// This first creates a temporary file and then rotates it in place.
///
/// `fsync`: use `fsync(2)` sycall to synchronize a file's in-core
/// state with storage device. This makes sure the is consistent even
/// aftert a power loss.
pub fn replace_file<P: AsRef<Path>>(
    path: P,
    data: &[u8],
    options: CreateOptions,
    fsync: bool,
) -> Result<(), Error> {
    let (mut file, tmp_path) = make_tmp_file(&path, options)?;

    if let Err(err) = file.write_all(data) {
        drop(file);
        let _ = unistd::unlink(&tmp_path);
        bail!("write failed: {}", err);
    }

    if fsync {
        // make sure data is on disk
        if let Err(err) = nix::unistd::fsync(file.as_raw_fd()) {
            drop(file);
            let _ = unistd::unlink(&tmp_path);
            bail!("fsync failed: {}", err);
        }
    }

    // Allow WORM file systems to commit the contents before the rename
    // and prevent temporary .fuse_hidden* files created by unlink.
    drop(file);

    if let Err(err) = std::fs::rename(&tmp_path, &path) {
        let _ = unistd::unlink(&tmp_path);
        bail!(
            "Atomic rename failed for file {:?} - {}",
            path.as_ref(),
            err
        );
    }

    Ok(())
}

/// Like open(2), but allows setting initial data, perm, owner and group
///
/// Since we need to initialize the file, we also need a solid slow
/// path where we create the file. In order to avoid races, we create
/// it in a temporary location and rotate it in place.
///
/// `fsync`: use `fsync(2)` sycall to synchronize the `initial_data`
/// to the storage device. This options has no effect it the `initial_data`
/// is empty or the file already exists.
pub fn atomic_open_or_create_file<P: AsRef<Path>>(
    path: P,
    mut oflag: OFlag,
    initial_data: &[u8],
    options: CreateOptions,
    fsync: bool,
) -> Result<File, Error> {
    let path = path.as_ref();

    #[cfg(target_os = "linux")]
    if oflag.contains(OFlag::O_TMPFILE) {
        bail!("open {:?} failed - unsupported OFlag O_TMPFILE", path);
    }

    if oflag.contains(OFlag::O_DIRECTORY) {
        bail!("open {:?} failed - unsupported OFlag O_DIRECTORY", path);
    }

    let exclusive = if oflag.contains(OFlag::O_EXCL) {
        oflag.remove(OFlag::O_EXCL); // we need to handle that ourselves
        true
    } else {
        false
    };

    oflag.remove(OFlag::O_CREAT); // we want to handle CREAT ourselves

    if !exclusive {
        // Note: 'mode' is ignored, because oflag does not contain O_CREAT or O_TMPFILE
        match nix::fcntl::open(path, oflag, stat::Mode::empty()) {
            Ok(fd) => return Ok(unsafe { File::from_raw_fd(fd) }),
            Err(err) => {
                if err.not_found() {
                    // fall through -  try to create the file
                } else {
                    bail!("open {:?} failed - {}", path, err);
                }
            }
        }
    }

    let (mut file, temp_file_name) = make_tmp_file(path, options)?;

    if !initial_data.is_empty() {
        file.write_all(initial_data).map_err(|err| {
            let _ = nix::unistd::unlink(&temp_file_name);
            format_err!(
                "writing initial data to {:?} failed - {}",
                temp_file_name,
                err,
            )
        })?;
        if fsync {
            // make sure the initial_data is on disk
            if let Err(err) = nix::unistd::fsync(file.as_raw_fd()) {
                let _ = nix::unistd::unlink(&temp_file_name);
                bail!(
                    "fsync of initial data to {:?} failed - {}",
                    temp_file_name,
                    err,
                )
            }
        }
    }

    if oflag.contains(OFlag::O_APPEND) {
        let result = fcntl(file.as_raw_fd(), FcntlArg::F_GETFL).and_then(|flags| {
            fcntl(
                file.as_raw_fd(),
                FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_APPEND),
            )
        });
        if let Err(err) = result {
            let _ = unistd::unlink(&temp_file_name);
            bail!("setting append mode on {:?} failed - {}", path, err);
        }
    }

    // rotate the file into place, but use `RENAME_NOREPLACE`, so in case 2 processes race against
    // the initialization, the first one wins!
    let rename_result = temp_file_name.with_nix_path(|c_file_name| {
        path.with_nix_path(|new_path| unsafe {
            // This also works on file systems which don't support hardlinks (eg. vfat)
            #[cfg(target_os = "linux")]
            let res = libc::renameat2(
                libc::AT_FDCWD,
                c_file_name.as_ptr(),
                libc::AT_FDCWD,
                new_path.as_ptr(),
                libc::RENAME_NOREPLACE,
            );
            #[cfg(target_os = "macos")]
            let res = libc::renameatx_np(
                libc::AT_FDCWD,
                c_file_name.as_ptr(),
                libc::AT_FDCWD,
                new_path.as_ptr(),
                libc::RENAME_EXCL,
            );
            match Errno::result(res) {
                Err(Errno::EINVAL) => (), // dumb file system, try `link`+`unlink`
                other => return other,
            };
            // but some file systems don't support `RENAME_NOREPLACE`
            // so we just use `link` + `unlink` instead
            let result = Errno::result(libc::link(c_file_name.as_ptr(), new_path.as_ptr()));
            let _ = libc::unlink(c_file_name.as_ptr());
            result
        })
    });

    match rename_result {
        Ok(Ok(Ok(_))) => Ok(file),
        Ok(Ok(Err(err))) => {
            // if another process has already raced ahead and created
            // the file, let's just open theirs instead:
            let _ = nix::unistd::unlink(&temp_file_name);

            if !exclusive && err.already_exists() {
                match nix::fcntl::open(path, oflag, stat::Mode::empty()) {
                    Ok(fd) => Ok(unsafe { File::from_raw_fd(fd) }),
                    Err(err) => bail!("open {:?} failed - {}", path, err),
                }
            } else {
                bail!(
                    "failed to move file at {:?} into place at {:?} - {}",
                    temp_file_name,
                    path,
                    err
                );
            }
        }
        Ok(Err(err)) => {
            let _ = nix::unistd::unlink(&temp_file_name);
            bail!("with_nix_path {:?} failed - {}", path, err);
        }
        Err(err) => {
            let _ = nix::unistd::unlink(&temp_file_name);
            bail!("with_nix_path {:?} failed - {}", temp_file_name, err);
        }
    }
}

// /usr/include/linux/fs.h: #define BLKGETSIZE64 _IOR(0x12,114,size_t)
// return device size in bytes (u64 *arg)
#[cfg(target_os = "linux")]
nix::ioctl_read!(blkgetsize64, 0x12, 114, u64);

// <sys/disk.h>: #define DKIOCGETBLOCKSIZE _IOR('d', 24, uint32_t)
//               #define DKIOCGETBLOCKCOUNT _IOR('d', 25, uint64_t)
#[cfg(target_os = "macos")]
nix::ioctl_read!(dkiocgetblocksize, b'd', 24, u32);
#[cfg(target_os = "macos")]
nix::ioctl_read!(dkiocgetblockcount, b'd', 25, u64);

/// Size of a block device in bytes.
#[cfg(target_os = "linux")]
fn block_device_size(file: &File) -> nix::Result<u64> {
    let mut size: u64 = 0;
    unsafe { blkgetsize64(file.as_raw_fd(), &mut size) }?;
    Ok(size)
}

/// Size of a block device in bytes.
#[cfg(target_os = "macos")]
fn block_device_size(file: &File) -> nix::Result<u64> {
    let mut block_size: u32 = 0;
    let mut block_count: u64 = 0;
    unsafe {
        dkiocgetblocksize(file.as_raw_fd(), &mut block_size)?;
        dkiocgetblockcount(file.as_raw_fd(), &mut block_count)?;
    }
    Ok(block_count * u64::from(block_size))
}

/// Return file or block device size
pub fn image_size(path: &Path) -> Result<u64, Error> {
    use std::os::unix::fs::FileTypeExt;

    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_type = metadata.file_type();

    if file_type.is_block_device() {
        match block_device_size(&file) {
            Ok(size) => Ok(size),
            Err(err) => bail!("getting block device size failed for {:?} - {}", path, err),
        }
    } else if file_type.is_file() {
        Ok(metadata.len())
    } else {
        bail!(
            "image size failed - got unexpected file type {:?}",
            file_type
        );
    }
}

#[cfg(feature = "timer")]
/// Create a file lock using fntl. This function allows you to specify
/// a timeout if you want to avoid infinite blocking.
///
/// With timeout set to 0, non-blocking mode is used and the function
/// will fail immediately if the lock can't be acquired.
pub fn lock_file<F: AsRawFd>(
    file: &mut F,
    exclusive: bool,
    timeout: Option<Duration>,
) -> Result<(), io::Error> {
    let lockarg = if exclusive {
        nix::fcntl::FlockArg::LockExclusive
    } else {
        nix::fcntl::FlockArg::LockShared
    };

    let timeout = match timeout {
        None => {
            nix::fcntl::flock(file.as_raw_fd(), lockarg).into_io_result()?;
            return Ok(());
        }
        Some(t) => t,
    };

    if timeout.as_nanos() == 0 {
        let lockarg = if exclusive {
            nix::fcntl::FlockArg::LockExclusiveNonblock
        } else {
            nix::fcntl::FlockArg::LockSharedNonblock
        };
        nix::fcntl::flock(file.as_raw_fd(), lockarg).into_io_result()?;
        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        // no per-thread timer signals here, poll with a non-blocking lock instead
        let lockarg = if exclusive {
            nix::fcntl::FlockArg::LockExclusiveNonblock
        } else {
            nix::fcntl::FlockArg::LockSharedNonblock
        };
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match nix::fcntl::flock(file.as_raw_fd(), lockarg) {
                Err(Errno::EWOULDBLOCK) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(Errno::EWOULDBLOCK) => return Err(io::Error::from_raw_os_error(libc::EINTR)),
                other => return other.into_io_result(),
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // unblock the timeout signal temporarily
        let _sigblock_guard = timer::unblock_timeout_signal();

        // setup a timeout timer
        let mut timer = timer::Timer::create(
            timer::Clock::Realtime,
            timer::TimerEvent::ThisThreadSignal(timer::SIGTIMEOUT),
        )?;

        timer.arm(
            timer::TimerSpec::new()
                .value(Some(timeout))
                .interval(Some(Duration::from_millis(10))),
        )?;

        nix::fcntl::flock(file.as_raw_fd(), lockarg).into_io_result()?;
        Ok(())
    }
}

#[cfg(feature = "timer")]
/// Open or create a lock file (append mode). Then try to
/// acquire a lock using `lock_file()`.
pub fn open_file_locked<P: AsRef<Path>>(
    path: P,
    timeout: Duration,
    exclusive: bool,
    options: CreateOptions,
) -> Result<File, Error> {
    let path = path.as_ref();

    let mut file = atomic_open_or_create_file(
        path,
        OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_APPEND,
        &[],
        options,
        false,
    )?;

    match lock_file(&mut file, exclusive, Some(timeout)) {
        Ok(_) => Ok(file),
        Err(err) => bail!("Unable to acquire lock {:?} - {}", path, err),
    }
}

/// Get an iterator over lines of a file, skipping empty lines and comments (lines starting with a
/// `#`).
pub fn file_get_non_comment_lines<P: AsRef<Path>>(
    path: P,
) -> Result<impl Iterator<Item = io::Result<String>>, Error> {
    let path = path.as_ref();

    Ok(io::BufReader::new(
        File::open(path).map_err(|err| format_err!("error opening {:?}: {}", path, err))?,
    )
    .lines()
    .filter_map(|line| match line {
        Ok(line) => {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                None
            } else {
                Some(Ok(line.to_string()))
            }
        }
        Err(err) => Some(Err(err)),
    }))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn atomic_open_or_create_file_keeps_append_mode() {
        use std::io::Seek;

        struct TestDir(PathBuf);
        impl Drop for TestDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let dir = TestDir(crate::fs::make_tmp_dir("./tests", None).unwrap());
        for (index, initial_data) in [b"".as_slice(), b"seed-"].into_iter().enumerate() {
            for append in [false, true] {
                let path = dir.0.join(format!("{index}-{append}"));
                let mut flags = OFlag::O_RDWR | OFlag::O_CLOEXEC;
                if append {
                    flags |= OFlag::O_APPEND;
                }
                let mut created = atomic_open_or_create_file(
                    &path,
                    flags,
                    initial_data,
                    CreateOptions::new(),
                    false,
                )
                .unwrap();
                let actual = fcntl(created.as_raw_fd(), FcntlArg::F_GETFL).unwrap();
                assert_eq!(
                    OFlag::from_bits_truncate(actual).contains(OFlag::O_APPEND),
                    append
                );

                let mut reopened = atomic_open_or_create_file(
                    &path,
                    flags | OFlag::O_APPEND,
                    b"ignored",
                    CreateOptions::new(),
                    false,
                )
                .unwrap();
                reopened.write_all(b"second").unwrap();
                created.rewind().unwrap();
                created.write_all(b"first").unwrap();

                let mut expected = [initial_data, b"second"].concat();
                if append {
                    expected.extend_from_slice(b"first");
                } else {
                    expected[..5].copy_from_slice(b"first");
                }
                assert_eq!(std::fs::read(&path).unwrap(), expected);
            }
        }
    }

    #[test]
    fn make_tmp_file_does_not_replace_extension() {
        let (_, path) = make_tmp_file("/tmp/proxmox-sys-test.json", CreateOptions::new()).unwrap();

        assert!(
            &path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("proxmox-sys-test.json")
        );

        let (_, path) = make_tmp_file(
            "/tmp/proxmox-sys-test.archive.1000.zst",
            CreateOptions::new(),
        )
        .unwrap();

        assert!(
            &path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("archive.1000.zst")
        );
    }
}
