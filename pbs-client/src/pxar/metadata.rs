use std::ffi::{CStr, CString};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::path::Path;

#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::{anyhow, Context, Error};
use nix::errno::Errno;
#[cfg(target_os = "linux")]
use nix::fcntl::OFlag;
#[cfg(target_os = "linux")]
use nix::sys::stat::Mode;

use pxar::Metadata;

use proxmox_sys::c_result;
use proxmox_sys::error::SysError;
use proxmox_sys::fs::xattr;
#[cfg(target_os = "linux")]
use proxmox_sys::fs::{self, acl};

use crate::pxar::tools::perms_from_metadata;
use crate::pxar::Flags;

//
// utility functions
//

fn allow_notsupp<E: SysError>(err: E) -> Result<(), E> {
    if err.is_errno(Errno::EOPNOTSUPP) || err.is_errno(Errno::ENOTSUP) {
        Ok(())
    } else {
        Err(err)
    }
}

fn allow_notsupp_remember<E: SysError>(err: E, not_supp: &mut bool) -> Result<(), E> {
    if err.is_errno(Errno::EOPNOTSUPP) || err.is_errno(Errno::ENOTSUP) {
        *not_supp = true;
        Ok(())
    } else {
        Err(err)
    }
}

fn timestamp_to_update_timespec(mtime: &pxar::format::StatxTimestamp) -> [libc::timespec; 2] {
    // restore mtime
    [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: mtime.secs,
            tv_nsec: mtime.nanos as _,
        },
    ]
}

//
// metadata application:
//

pub fn apply_at(
    flags: Flags,
    metadata: &Metadata,
    parent: RawFd,
    file_name: &CStr,
    path_info: &Path,
    on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        let fd = proxmox_sys::fd::openat(
            &parent,
            file_name,
            OFlag::O_PATH | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )?;

        apply(flags, metadata, fd.as_raw_fd(), path_info, on_error)
    }

    // no O_PATH, so address the entry by path to also cover symlinks, FIFOs and devices
    #[cfg(not(target_os = "linux"))]
    {
        let mut path = fd_path(parent)?.into_bytes();
        path.push(b'/');
        path.extend_from_slice(file_name.to_bytes());
        let c_path = CString::new(path)?;

        apply_to_path(flags, metadata, &c_path, None, path_info, on_error)
    }
}

/// Path referring to the file behind `fd`, usable with path based syscalls.
#[cfg(target_os = "linux")]
fn fd_path(fd: RawFd) -> Result<CString, Error> {
    Ok(CString::new(format!("/proc/self/fd/{}", fd)).unwrap())
}

#[cfg(target_os = "macos")]
fn fd_path(fd: RawFd) -> Result<CString, Error> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    c_result!(unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) })
        .context("failed to get path of file descriptor")?;
    Ok(CStr::from_bytes_until_nul(&buf)?.to_owned())
}

// The initial flags are Linux file attributes which have to be set before writing data, there
// is nothing comparable on macOS.
#[cfg(not(target_os = "linux"))]
pub fn apply_initial_flags(
    _flags: Flags,
    _metadata: &Metadata,
    _fd: RawFd,
    _on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn apply_initial_flags(
    flags: Flags,
    metadata: &Metadata,
    fd: RawFd,
    on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    let entry_flags = Flags::from_bits_truncate(metadata.stat.flags);
    apply_chattr(
        fd,
        entry_flags.to_initial_chattr(),
        flags.to_initial_chattr(),
    )
    .or_else(on_error)?;
    Ok(())
}

pub fn apply(
    flags: Flags,
    metadata: &Metadata,
    fd: RawFd,
    path_info: &Path,
    on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    let c_path = fd_path(fd)?;
    apply_to_path(flags, metadata, &c_path, Some(fd), path_info, on_error)
}

/// Apply the metadata to the file at `c_proc_path`, which must not be followed if it is a
/// symlink. `fd` is required for Linux specific attributes.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn apply_to_path(
    flags: Flags,
    metadata: &Metadata,
    c_proc_path: &CStr,
    fd: Option<RawFd>,
    path_info: &Path,
    on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    apply_ownership(flags, c_proc_path.as_ptr(), metadata, &mut *on_error)?;

    let mut skip_xattrs = false;
    apply_xattrs(flags, c_proc_path.as_ptr(), metadata, &mut skip_xattrs)
        .or_else(&mut *on_error)?;
    add_fcaps(flags, c_proc_path.as_ptr(), metadata, &mut skip_xattrs).or_else(&mut *on_error)?;
    apply_acls(flags, c_proc_path, metadata, path_info)
        .context("failed to apply acls")
        .or_else(&mut *on_error)?;
    #[cfg(target_os = "linux")]
    if let Some(fd) = fd {
        apply_quota_project_id(flags, fd, metadata).or_else(&mut *on_error)?;
    }

    // Finally mode and time. We may lose access with mode, but the changing the mode also
    // affects times.
    if !metadata.is_symlink() && flags.contains(Flags::WITH_PERMISSIONS) {
        c_result!(unsafe {
            libc::chmod(c_proc_path.as_ptr(), perms_from_metadata(metadata)?.bits())
        })
        .map(drop)
        .or_else(allow_notsupp)
        .context("failed to change file mode")
        .or_else(&mut *on_error)?;
    }

    let res = c_result!(unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c_proc_path.as_ptr(),
            timestamp_to_update_timespec(&metadata.stat.mtime).as_ptr(),
            UTIMENSAT_FLAGS,
        )
    });
    match res {
        Ok(_) => (),
        Err(ref err) if err.is_errno(Errno::EOPNOTSUPP) || err.is_errno(Errno::ENOTSUP) => (),
        Err(err) => {
            on_error(anyhow!(err).context(format!(
                "failed to restore mtime attribute on {path_info:?}"
            )))?;
        }
    }

    if metadata.stat.flags != 0 {
        #[cfg(target_os = "linux")]
        if let Some(fd) = fd {
            apply_flags(flags, fd, metadata.stat.flags).or_else(&mut *on_error)?;
        }
        #[cfg(target_os = "macos")]
        apply_flags(flags, c_proc_path, metadata.stat.flags).or_else(&mut *on_error)?;
    }

    Ok(())
}

// The magic links in /proc/self/fd refer to the file itself, elsewhere we must not follow
// symlinks explicitly.
#[cfg(target_os = "linux")]
const UTIMENSAT_FLAGS: libc::c_int = 0;
#[cfg(not(target_os = "linux"))]
const UTIMENSAT_FLAGS: libc::c_int = libc::AT_SYMLINK_NOFOLLOW;

#[cfg(target_os = "linux")]
unsafe fn sys_chown(path: *const libc::c_char, uid: u32, gid: u32) -> libc::c_int {
    libc::chown(path, uid, gid)
}

#[cfg(not(target_os = "linux"))]
unsafe fn sys_chown(path: *const libc::c_char, uid: u32, gid: u32) -> libc::c_int {
    libc::lchown(path, uid, gid)
}

#[cfg(target_os = "linux")]
unsafe fn sys_setxattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::setxattr(path, name, value, size, 0)
}

#[cfg(target_os = "macos")]
unsafe fn sys_setxattr(
    path: *const libc::c_char,
    name: *const libc::c_char,
    value: *const libc::c_void,
    size: usize,
) -> libc::c_int {
    libc::setxattr(path, name, value, size, 0, libc::XATTR_NOFOLLOW)
}

pub fn apply_ownership(
    flags: Flags,
    c_proc_path: *const libc::c_char,
    metadata: &Metadata,
    on_error: &mut (dyn FnMut(Error) -> Result<(), Error> + Send),
) -> Result<(), Error> {
    if !flags.contains(Flags::WITH_OWNER) {
        return Ok(());
    }
    unsafe {
        // UID and GID first, as this fails if we lose access anyway.
        c_result!(sys_chown(c_proc_path, metadata.stat.uid, metadata.stat.gid))
            .map(drop)
            .or_else(allow_notsupp)
            .context("failed to set ownership")
            .or_else(&mut *on_error)?;
    }
    Ok(())
}

fn add_fcaps(
    flags: Flags,
    c_proc_path: *const libc::c_char,
    metadata: &Metadata,
    skip_xattrs: &mut bool,
) -> Result<(), Error> {
    if *skip_xattrs || !flags.contains(Flags::WITH_FCAPS) {
        return Ok(());
    }
    // file capabilities only exist on Linux
    if cfg!(not(target_os = "linux")) {
        return Ok(());
    }
    let fcaps = match metadata.fcaps.as_ref() {
        Some(fcaps) => fcaps,
        None => return Ok(()),
    };

    c_result!(unsafe {
        sys_setxattr(
            c_proc_path,
            xattr::xattr_name_fcaps().as_ptr(),
            fcaps.data.as_ptr() as *const libc::c_void,
            fcaps.data.len(),
        )
    })
    .map(drop)
    .or_else(|err| allow_notsupp_remember(err, skip_xattrs))
    .context("failed to apply file capabilities")
}

fn apply_xattrs(
    flags: Flags,
    c_proc_path: *const libc::c_char,
    metadata: &Metadata,
    skip_xattrs: &mut bool,
) -> Result<(), Error> {
    if *skip_xattrs || !flags.contains(Flags::WITH_XATTRS) {
        return Ok(());
    }

    for xattr in &metadata.xattrs {
        if *skip_xattrs {
            return Ok(());
        }

        if !xattr::is_valid_xattr_name(xattr.name()) {
            log::info!("skipping invalid xattr named {:?}", xattr.name());
            continue;
        }

        c_result!(unsafe {
            sys_setxattr(
                c_proc_path,
                xattr.name().as_ptr() as *const libc::c_char,
                xattr.value().as_ptr() as *const libc::c_void,
                xattr.value().len(),
            )
        })
        .map(drop)
        .or_else(|err| allow_notsupp_remember(err, &mut *skip_xattrs))
        .context("failed to apply extended attributes")?;
    }

    Ok(())
}

// POSIX ACLs are not available on macOS, they are not restored there.
#[cfg(not(target_os = "linux"))]
fn apply_acls(
    _flags: Flags,
    _c_proc_path: &CStr,
    _metadata: &Metadata,
    _path_info: &Path,
) -> Result<(), Error> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_acls(
    flags: Flags,
    c_proc_path: &CStr,
    metadata: &Metadata,
    path_info: &Path,
) -> Result<(), Error> {
    if !flags.contains(Flags::WITH_ACL) || metadata.acl.is_empty() {
        return Ok(());
    }

    let mut acl = acl::ACL::init(5)?;

    // acl type access:
    acl.add_entry_full(
        acl::ACL_USER_OBJ,
        None,
        acl::mode_user_to_acl_permissions(metadata.stat.mode),
    )?;

    acl.add_entry_full(
        acl::ACL_OTHER,
        None,
        acl::mode_other_to_acl_permissions(metadata.stat.mode),
    )?;

    match metadata.acl.group_obj.as_ref() {
        Some(group_obj) => {
            acl.add_entry_full(
                acl::ACL_MASK,
                None,
                acl::mode_group_to_acl_permissions(metadata.stat.mode),
            )?;
            acl.add_entry_full(acl::ACL_GROUP_OBJ, None, group_obj.permissions.0)?;
        }
        None => {
            let mode = acl::mode_group_to_acl_permissions(metadata.stat.mode);

            acl.add_entry_full(acl::ACL_GROUP_OBJ, None, mode)?;

            if !metadata.acl.users.is_empty() || !metadata.acl.groups.is_empty() {
                log::warn!(
                    "Warning: {:?}: Missing GROUP_OBJ entry in ACL, resetting to value of MASK",
                    path_info,
                );
                acl.add_entry_full(acl::ACL_MASK, None, mode)?;
            }
        }
    }

    for user in &metadata.acl.users {
        acl.add_entry_full(acl::ACL_USER, Some(user.uid), user.permissions.0)?;
    }

    for group in &metadata.acl.groups {
        acl.add_entry_full(acl::ACL_GROUP, Some(group.gid), group.permissions.0)?;
    }

    if !acl.is_valid() {
        bail!("Error while restoring ACL - ACL invalid");
    }

    acl.set_file(c_proc_path, acl::ACL_TYPE_ACCESS)?;
    drop(acl);

    // acl type default:
    if let Some(default) = metadata.acl.default.as_ref() {
        let mut acl = acl::ACL::init(5)?;

        acl.add_entry_full(acl::ACL_USER_OBJ, None, default.user_obj_permissions.0)?;

        acl.add_entry_full(acl::ACL_GROUP_OBJ, None, default.group_obj_permissions.0)?;

        acl.add_entry_full(acl::ACL_OTHER, None, default.other_permissions.0)?;

        if default.mask_permissions != pxar::format::acl::Permissions::NO_MASK {
            acl.add_entry_full(acl::ACL_MASK, None, default.mask_permissions.0)?;
        }

        for user in &metadata.acl.default_users {
            acl.add_entry_full(acl::ACL_USER, Some(user.uid), user.permissions.0)?;
        }

        for group in &metadata.acl.default_groups {
            acl.add_entry_full(acl::ACL_GROUP, Some(group.gid), group.permissions.0)?;
        }

        if !acl.is_valid() {
            bail!("Error while restoring ACL - ACL invalid");
        }

        acl.set_file(c_proc_path, acl::ACL_TYPE_DEFAULT)?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_quota_project_id(flags: Flags, fd: RawFd, metadata: &Metadata) -> Result<(), Error> {
    if !flags.contains(Flags::WITH_QUOTA_PROJID) {
        return Ok(());
    }

    let projid = match metadata.quota_project_id {
        Some(projid) => projid,
        None => return Ok(()),
    };

    let mut fsxattr = fs::FSXAttr::default();
    unsafe {
        fs::fs_ioc_fsgetxattr(fd, &mut fsxattr)
            .context("error while getting fsxattr to restore quota project id")?;

        fsxattr.fsx_projid = projid.projid as u32;

        fs::fs_ioc_fssetxattr(fd, &fsxattr)
            .context("error while setting fsxattr to restore quota project id")?;
    }

    Ok(())
}

pub(crate) fn errno_is_unsupported(errno: Errno) -> bool {
    matches!(
        errno,
        Errno::ENOTTY | Errno::ENOSYS | Errno::EBADF | Errno::EOPNOTSUPP | Errno::EINVAL
    ) || errno == Errno::ENOTSUP
}

#[cfg(target_os = "linux")]
fn apply_chattr(fd: RawFd, chattr: libc::c_long, mask: libc::c_long) -> Result<(), Error> {
    if chattr == 0 {
        return Ok(());
    }

    let mut fattr: libc::c_long = 0;
    match unsafe { fs::read_attr_fd(fd, &mut fattr) } {
        Ok(_) => (),
        Err(errno) if errno_is_unsupported(errno) => {
            return Ok(());
        }
        Err(err) => return Err(err).context("failed to read file attributes"),
    }

    let attr = (chattr & mask) | (fattr & !mask);

    if attr == fattr {
        return Ok(());
    }

    match unsafe { fs::write_attr_fd(fd, &attr) } {
        Ok(_) => Ok(()),
        Err(errno) if errno_is_unsupported(errno) => Ok(()),
        Err(err) => Err(err).context("failed to set file attributes"),
    }
}

#[cfg(target_os = "linux")]
fn apply_flags(flags: Flags, fd: RawFd, entry_flags: u64) -> Result<(), Error> {
    let entry_flags = Flags::from_bits_truncate(entry_flags);

    apply_chattr(fd, entry_flags.to_chattr(), flags.to_chattr())?;

    let fatattr = (flags & entry_flags).to_fat_attr();
    if fatattr != 0 {
        match unsafe { fs::write_fat_attr_fd(fd, &fatattr) } {
            Ok(_) => (),
            Err(errno) if errno_is_unsupported(errno) => (),
            Err(err) => return Err(err).context("failed to set file FAT attributes"),
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_flags(flags: Flags, c_path: &CStr, entry_flags: u64) -> Result<(), Error> {
    let entry_flags = Flags::from_bits_truncate(entry_flags);
    let mask = flags.to_st_flags();

    let stat = nix::sys::stat::lstat(c_path).context("failed to read file flags")?;
    let st_flags = (entry_flags.to_st_flags() & mask) | (stat.st_flags & !mask);
    if st_flags == stat.st_flags {
        return Ok(());
    }

    match c_result!(unsafe { libc::lchflags(c_path.as_ptr(), st_flags) }) {
        Ok(_) => Ok(()),
        Err(err) if errno_is_unsupported(Errno::from_i32(err.raw_os_error().unwrap_or(0))) => {
            Ok(())
        }
        Err(err) => Err(err).context("failed to set file flags"),
    }
}
