#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![deny(unsafe_op_in_unsafe_fn)]

use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::LazyLock;

pub mod boot_mode;
pub mod command;
// links against libcrypt (crypt_r, crypt_gensalt_rn)
#[cfg(all(feature = "crypt", target_os = "linux"))]
pub mod crypt;
pub mod error;
pub mod fd;
pub mod fs;
pub mod linux;
#[cfg(feature = "logrotate")]
pub mod logrotate;
pub mod macros;
pub mod mmap;
pub mod process_locker;
pub mod systemd;

/// Returns the hosts node name (UTS node name)
pub fn nodename() -> &'static str {
    static NODENAME: LazyLock<String> = LazyLock::new(|| {
        std::str::from_utf8(
            nix::sys::utsname::uname()
                .expect("failed to get nodename")
                .nodename()
                .as_bytes(),
        )
        .expect("non utf-8 nodename not supported")
        .split('.')
        .next()
        .unwrap()
        .to_owned()
    });

    &NODENAME
}

/// Wrapper for `nix::unistd::pipe2` defaulting to `O_CLOEXEC`.
pub fn pipe() -> Result<(OwnedFd, OwnedFd), nix::Error> {
    #[cfg(target_os = "linux")]
    {
        nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC)
    }

    // no pipe2(2) on macOS, set FD_CLOEXEC afterwards
    #[cfg(not(target_os = "linux"))]
    {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        use std::os::fd::AsRawFd;
        let (pin, pout) = nix::unistd::pipe()?;
        fcntl(pin.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        fcntl(pout.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        Ok((pin, pout))
    }
}
