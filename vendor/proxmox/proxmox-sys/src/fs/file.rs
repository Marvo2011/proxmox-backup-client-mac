use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
#[cfg(feature = "timer")]
use std::time::Duration;

use anyhow::{bail, format_err, Error};
use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::sys::stat;
use nix::unistd;
use nix::NixPath;
use serde_json::Value;

use proxmox_lang::try_block;

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
/// 'Err' if the file dose not exist.
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
/// 'Err' if the file dose not exist.
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

    let raw = match std::fs::read(path) {
        Ok(v) => v,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                if let Some(v) = default {
                    return Ok(v);
                }
            }
            bail!("unable to read json {:?} - {}", path, err);
        }
    };

    try_block!({
        let data = String::from_utf8(raw)?;
        let json = serde_json::from_str(&data)?;
        Ok(json)
    })
    .map_err(|err: Error| format_err!("unable to parse json from {:?} - {}", path, err))
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

/// Takes a Path and CreateOptions, creates a tmpfile from it and returns
/// a RawFd and PathBuf for it
pub fn make_tmp_file<P: AsRef<Path>>(
    path: P,
    options: CreateOptions,
) -> Result<(File, PathBuf), Error> {
    let path = path.as_ref();

    // use mkstemp here, because it works with different processes, threads, even tokio tasks
    let mut template = path.to_owned();
    template.set_extension("tmp_XXXXXX");
    let (mut file, tmp_path) = match unistd::mkstemp(&template) {
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
    let (fd, tmp_path) = make_tmp_file(&path, options)?;

    let mut file = unsafe { File::from_raw_fd(fd.into_raw_fd()) };

    if let Err(err) = file.write_all(data) {
        let _ = unistd::unlink(&tmp_path);
        bail!("write failed: {}", err);
    }

    if fsync {
        // make sure data is on disk
        if let Err(err) = nix::unistd::fsync(file.as_raw_fd()) {
            let _ = unistd::unlink(&tmp_path);
            bail!("fsync failed: {}", err);
        }
    }

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
        oflag.remove(OFlag::O_EXCL); // we nned to handle that ourselfes
        true
    } else {
        false
    };

    oflag.remove(OFlag::O_CREAT); // we want to handle CREAT ourselfes

    if !exclusive {
        // Note: 'mode' is ignored, because oflag does not contain O_CREAT or O_TMPFILE
        match nix::fcntl::open(path, oflag, stat::Mode::empty()) {
            Ok(fd) => return Ok(unsafe { File::from_raw_fd(fd) }),
            Err(err) => {
                if err.not_found() {
                    // fall thrue -  try to create the file
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
