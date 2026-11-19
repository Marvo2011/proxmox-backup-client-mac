# Changelog

The version is the upstream proxmox-backup version plus the release date of the
port. The release workflow takes the release notes from the section of the
released version, so every version needs one.

## 4.2.10+20261119

- Based on upstream proxmox-backup 4.2.10 (before 3.2.4). New in the client
  since then, among others:
  - `--change-detection-mode metadata`: split archives (`.mpxar`/`.ppxar`),
    unchanged files are reused from the previous snapshot instead of being
    read again
  - the repository can be given as separate options (`--server`, `--port`,
    `--datastore`, `--auth-id`) instead of `--repository`
  - the limit for open files is raised for the metadata change detection
    (on macOS up to the kernel limit `kern.maxfilesperproc`)
- The fixes of 3.2.4+20261119 (online-only cloud files, `EINTR`, upload
  errors) are carried over.
- Restoring hardlinks refuses `..` and symlinks in the target path, as
  `openat2(RESOLVE_BENEATH)` does on Linux.
- Building needs Rust 1.91 or newer.

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
