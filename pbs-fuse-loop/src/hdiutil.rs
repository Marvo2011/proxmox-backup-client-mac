//! Attach image files as disk devices with hdiutil(1), the macOS counterpart to loop devices

use std::path::Path;
use std::process::Command;

use anyhow::{format_err, Error};

/// Attach a raw image file read-only, without mounting any file system on it.
///
/// Returns the path of the whole disk device, e.g. `/dev/disk4`.
pub fn attach(image: &Path) -> Result<String, Error> {
    let mut command = Command::new("hdiutil");
    command
        .args([
            "attach",
            "-readonly",
            "-nomount",
            "-noverify",
            "-noautofsck",
        ])
        .args(["-imagekey", "diskimage-class=CRawDiskImage"])
        .arg(image);
    let output = proxmox_sys::command::run_command(command, None)?;

    // the first line is the whole disk, followed by its partitions
    output
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .filter(|device| device.starts_with("/dev/disk"))
        .map(str::to_owned)
        .ok_or_else(|| format_err!("unexpected output of hdiutil attach: {:?}", output))
}

/// Detach a disk device attached with `attach`.
pub fn detach(device: &str, force: bool) -> Result<(), Error> {
    let mut command = Command::new("hdiutil");
    command.arg("detach");
    if force {
        command.arg("-force");
    }
    command.arg(device);
    proxmox_sys::command::run_command(command, None)?;
    Ok(())
}
