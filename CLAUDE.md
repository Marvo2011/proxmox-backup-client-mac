# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A macOS (Apple Silicon) port of the Proxmox Backup client (`proxmox-backup-client`) and the `pxar` tool. It is trimmed from the upstream proxmox-backup 3.2.4 workspace (commit `4940514b`): only the crates the client needs are included, and the server, web UI, docs and Debian packaging are not.

## Commands

```sh
cargo build --release -p proxmox-backup-client -p pxar-bin
cargo test -p pbs-client -p pbs-datastore -p pbs-key-config -p pbs-pxar-fuse -p pxar-bin
cargo test -p pbs-datastore --test index_mmap        # single integration test
# vendored crates are not workspace members, test them via their manifest:
cargo test --manifest-path vendor/proxmox/Cargo.toml -p proxmox-sys
cargo fmt                                            # rustfmt.toml: edition 2021
cargo clippy
```

The build needs FUSE-T installed (https://www.fuse-t.org), because `proxmox-fuse`'s `glue.c` uses its libfuse3 headers. `.cargo/config.toml` links libfuse3 weakly (`-weak-lfuse3`, rpath `/usr/local/lib`), so the binaries run without FUSE-T and only `mount`/`map` require it (`proxmox_fuse::sys::available()`). Releases link OpenSSL statically (`OPENSSL_STATIC=1`, see `.github/workflows/macos-release.yml`), and CI checks that only system libraries are linked.

## Layout

- `vendor/`: Proxmox crates (proxmox-sys, proxmox-time, pxar, proxmox-fuse, …) patched in via `.cargo/config.toml`, most with macOS changes. `vendor/README.md` lists the upstream commits and every local change. Keep it updated when changing vendored code.
- `pbs-client`: backup protocol client and the pxar archiver (`src/pxar/create.rs`, `extract.rs`, `metadata.rs`, `flags.rs`).
- `pbs-datastore`: index/blob formats used by the client. `pbs-config` and the datastore server paths compile here but are not used. Linux-only parts are stubbed (`config_version_cache_stub.rs`, `task_tracking.rs`).
- `pbs-pxar-fuse` (`mount`) and `pbs-fuse-loop` (`map`, using `hdiutil` on macOS in `src/hdiutil.rs`).
- `proxmox-backup-client`, `pxar-bin`: the binaries.

## Porting conventions

- Platform code uses `#[cfg(target_os = "linux")]` / `"macos"`, and the Linux code paths stay unchanged. This keeps the changes reviewable against upstream and makes it possible to send them upstream.
- Archive format compatibility with Linux matters: archives created on macOS must restore on Linux and vice versa.
- macOS specifics that came up: `mode_t` is `u16`, `dev_t` is `i32` (8 bit major / 24 bit minor), `ENOTSUP` != `EOPNOTSUPP`, `ENOATTR` instead of `ENODATA`, 16 KiB pages (mmap offsets must be page aligned), no `/proc`, `O_PATH`, `O_TMPFILE`, `statfs64`, `renameat2`, `epoll`.
- Metadata is restored through `F_GETPATH` paths with no-follow syscalls (`metadata.rs`). The archiver only opens regular files and directories (`create.rs`), and other file types get stat-only metadata.
- FUSE-T peculiarities, handled in `proxmox-fuse` and `pbs-pxar-fuse`:
  - It retries `EAGAIN` inside `fuse_session_receive_buf`, so input is checked with `poll` first.
  - It looks up `.`/`..` explicitly.
  - It presents names in NFD, so lookups fall back to normalized comparison.
  - It cannot mount on paths containing `:`.
  - Its `fuse_buf` has the libfuse 3.17 `mem_size` field.
