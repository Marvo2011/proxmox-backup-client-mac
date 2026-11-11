# Proxmox Backup Client for macOS

A port of the [Proxmox Backup Server](https://www.proxmox.com/en/proxmox-backup-server)
command line client (`proxmox-backup-client`) and the `pxar` archive tool to
macOS on Apple Silicon.

It is based on upstream proxmox-backup 3.2.4 (commit
`4940514b0f05d6cd6a5f711edfdd47c1fa41b537`) and contains only the crates needed
for the client. This is an unofficial port and not affiliated with Proxmox
Server Solutions GmbH.

## Installation

Download the archive of the latest
[release](https://github.com/Marvo2011/proxmox-backup-client-mac/releases),
unpack it and put `proxmox-backup-client` and `pxar` somewhere in your `PATH`.

The binaries are not notarized. After downloading with a browser, remove the
quarantine attribute:

```sh
xattr -d com.apple.quarantine proxmox-backup-client pxar
```

`mount`, `map` and `unmap` need [FUSE-T](https://www.fuse-t.org) (no kernel
extension required), all other commands run without it.

## Differences to Linux

- `map` attaches drive images with `hdiutil` as `/dev/diskN` (read-only, no
  file system mounted). `unmap` takes the device, its number or the archive
  name.
- Not archived or restored: POSIX ACLs, file capabilities, quota project IDs
  and Linux file attributes. macOS file flags (hidden, immutable, append,
  nodump) map onto the corresponding pxar flags.
- Only `user.*` and `trusted.*` extended attributes are archived, so
  `com.apple.*` attributes are skipped.
- Mounted archives (FUSE-T, NFS based) do not show extended attributes or file
  flags, and setuid bits are stripped.
- Block devices can be backed up via `/dev/diskN`, `/dev/rdiskN` is not
  supported yet.

## Building

Requirements: Rust, OpenSSL (e.g. `brew install openssl@3`) and FUSE-T (its
headers and library are needed for building).

```sh
cargo build --release -p proxmox-backup-client -p pxar-bin
cargo test -p pbs-client -p pbs-datastore
```

The Proxmox crates are vendored in `vendor/` together with their macOS changes,
see `vendor/README.md`.

Pushes to `master` build a release via GitHub Actions
(`.github/workflows/macos-release.yml`), tagged with the client version.

## License

AGPL-3.0, see `LICENSE` and `COPYRIGHT`. Copyright (C) 2019 - 2024 Proxmox
Server Solutions GmbH, macOS changes by the contributors of this repository.
