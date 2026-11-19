# Vendored crates for the macOS client build

These crates are patched in via `.cargo/config.toml`. They are not on crates.io (or not in a usable version), and several need
changes to build and work on macOS.

| Directory | Upstream | Commit |
|-----------|----------|--------|
| `proxmox/` | https://git.proxmox.com/git/proxmox.git | `17d7f43dbca69d622bee07a93626ae1e741f7109` (2026-11-05) |
| `pxar/` | https://git.proxmox.com/git/pxar.git | `091a8a382d0d6fc71025351fb35c51b1f3b0074d` (1.0.1) |
| `pathpatterns/` | https://git.proxmox.com/git/pathpatterns.git | `5323cbe49ae5d592eb8a3fa2e215550e83dd7fba` (1.0.0) |
| `proxmox-fuse/` | https://git.proxmox.com/git/proxmox-fuse.git | `ac99ac97f7c2eb7ab9ee6ec3b41034e68b1eca7d` (3.0.0) |

The commits are the latest ones before the proxmox-backup release the client is
based on (see the top level `README.md`).

Only the crates of the `proxmox` workspace that this repository depends on are
copied, and the `members` list of `proxmox/Cargo.toml` is reduced accordingly.
`pbs-api-types` is one of them, it moved from proxmox-backup to proxmox.git.
`debian/` directories are left out.

## Local changes

Platform specific code uses `#[cfg(target_os = ...)]` and keeps the Linux code
paths unchanged.

- `proxmox-sys`
  - xattr calls: macOS variants take extra `position`/`options` arguments,
    `flistxattr`/`fgetxattr` retry on `EINTR` (macOS returns it without a signal)
  - `fs_info`: `statfs` instead of `statfs64`
  - `atomic_open_or_create_file`: `renameatx_np(RENAME_EXCL)` instead of
    `renameat2(RENAME_NOREPLACE)`, `O_TMPFILE` check Linux only
  - `make_tmp_file`: declares `mkostemp` itself on macOS, the libc crate has
    no binding for it there
  - `lock_file` with timeout: polls a non-blocking `flock` instead of using a
    per-thread timer signal
  - `image_size`: block device size via `DKIOCGETBLOCKCOUNT` and
    `DKIOCGETBLOCKSIZE` instead of `BLKGETSIZE64`
  - `pipe` (no `pipe2`), `fill_with_random_data`, `set_tcp_keepalive`,
    `process_locker`: macOS equivalents
  - Linux only: `linux::{pid, procfs, timer}`, `fs::fsx_attr` (Linux ioctls),
    `fs::acl` (libacl), `crypt` (libcrypt)
  - `Mmap::map_fd`: page aligns the mmap(2) offset, index files map their entries at offset
    4096, which fails with EINVAL where pages are larger (16 KiB on Apple Silicon)
- `proxmox-fuse` (FUSE-T on macOS, which provides libfuse3)
  - reactor uses `kqueue` instead of `epoll`/`eventfd`, an unmount shows up
    as `EV_EOF` on FUSE-T's socket
  - `FuseBuf` has the `mem_size` field of libfuse >= 3.17, which FUSE-T is
    based on (it writes the whole struct)
  - FUSE-T retries on `EAGAIN` inside `fuse_session_receive_buf` (busy
    loop), so pending input is checked with `poll` before calling it
  - libfuse3 is linked weakly (see `.cargo/config.toml`), `Fuse::builder`
    fails with a clear error if FUSE-T is not installed
- `proxmox-async`: no `#[link(name = "crypto")]` on macOS, it pulled in the
  shared libcrypto although openssl-sys links it statically for releases
- `proxmox-time`: `tm_zone` pointer type, RFC3339 formatting without the
  glibc-only `%010F`, arithmetic `timegm` (the macOS one fails before 1900)
- `proxmox-uuid`: no `libuuid` to link on macOS (part of libSystem)
- `proxmox-compression`: `major`/`minor` types
- `proxmox-shared-memory`: only the server uses it (shared rate limiter of the
  S3 client), it needs tmpfs, `O_TMPFILE` and robust mutexes. On macOS opening
  shared memory fails with an error, the rest only compiles.
- `proxmox-systemd`: libsystemd bindings (`journal`, `notify`, `sd_id128`) are
  Linux only, the unit name escaping used by the client is available everywhere
- `pxar`: `Device::{to,from}_dev_t` for the macOS `dev_t` layout, libc
  dependency on macOS
- `pathpatterns`: `mode_t` is `u16` on macOS

## Updating

Copy the crates from the new upstream commit, update the table above and
reapply the changes listed here (`git diff` against the previous vendored
version shows them).
