# Changelog

The version is the upstream proxmox-backup version plus the release date of the
port. The release workflow takes the release notes from the section of the
released version, so every version needs one.

## 3.2.4+20261119

- Online-only files of cloud storage folders (iCloud Drive, Nextcloud, OneDrive
  and other File Provider folders in `~/Library/CloudStorage`) are skipped with
  the warning `skipping online-only file: …` instead of being downloaded. Before,
  reading them downloaded the whole file and a failed download (e.g.
  `Operation timed out (os error 60)`) aborted the backup. Files that are kept
  on the Mac ("Always keep on this device") are backed up as before.
- System calls that macOS interrupts with `EINTR` without a signal are retried.
  Before, the backup could abort with `EINTR: Interrupted system call`, e.g. in
  temporary folders under `~/Library/Containers`.
- Upload errors report their actual cause. Before, a failed backup often only
  showed `catalog upload error - channel closed`.
- The client no longer keeps reading the source and hangs after the upload to
  the server has failed.
- `proxmox-backup-client version` shows the full version, e.g.
  `3.2.4+20261119`.
- Installation with Homebrew: `brew install marvo2011/tap/proxmox-backup-client`.

## 3.2.4

- First release of the macOS port, based on upstream proxmox-backup 3.2.4.
