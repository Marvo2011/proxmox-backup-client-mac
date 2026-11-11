# Vendored crates for the macOS client build

These crates are patched in via `.cargo/config.toml`. They are not on crates.io (or not in a usable version), and several need
changes to build and work on macOS.

| Directory | Upstream | Commit |
|-----------|----------|--------|
| `proxmox/` | https://git.proxmox.com/git/proxmox.git | `f7a22604ae259915a8d43e7c75843b4d46dce80f` (2024-06-06) |
| `pxar/` | https://git.proxmox.com/git/pxar.git | `29cbeed3e1b52f5eef455cdfa8b5e93f4e3e88f5` (0.10.2) |
| `pathpatterns/` | https://git.proxmox.com/git/pathpatterns.git | `281894a5b66099e919d167cd5f0644fff6aca234` |
| `proxmox-fuse/` | https://git.proxmox.com/git/proxmox-fuse.git | `dc151753b3ef3661b171f0439aa60f59e3568395` |

Only the crates of the `proxmox` workspace that this repository depends on are
copied, and the `members` list of `proxmox/Cargo.toml` is reduced accordingly.
`debian/` directories are left out.

## Local changes

Platform specific code uses `#[cfg(target_os = ...)]` and keeps the Linux code
paths unchanged.

- `proxmox-sys`
  - xattr calls: macOS variants take extra `position`/`options` arguments
  - `fs_info`: `statfs` instead of `statfs64`
  - `atomic_open_or_create_file`: `renameatx_np(RENAME_EXCL)` instead of
    `renameat2(RENAME_NOREPLACE)`, `O_TMPFILE` check Linux only
  - `lock_file` with timeout: polls a non-blocking `flock` instead of using a
    per-thread timer signal
  - `image_size`: block device size via `DKIOCGETBLOCKCOUNT` and
    `DKIOCGETBLOCKSIZE` instead of `BLKGETSIZE64`
  - `pipe`, `fill_with_random_data`, `set_tcp_keepalive`, `process_locker`:
    macOS equivalents
  - Linux only: `linux::{pid, procfs, timer}`, `fs::fsx_attr` (Linux ioctls),
    `fs::acl` (libacl), `crypt` (libcrypt)
  - `Mmap::map_fd`: page aligns the mmap(2) offset, index files map their entries at offset
    4096, which fails with EINVAL where pages are larger (16 KiB on Apple Silicon)
  - `create_path`: no longer wraps `AT_FDCWD` in an `OwnedFd`, dropping it
    called `close(AT_FDCWD)` (aborts with debug builds of recent Rust
    versions; not macOS specific)
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
- `proxmox-lang`: `offsetof!` uses `std::mem::offset_of!`, the previous null
  pointer dereference is undefined behavior (not macOS specific)
- `proxmox-compression`: `major`/`minor` types
- `pxar`: `Device::{to,from}_dev_t` for the macOS `dev_t` layout, libc
  dependency on macOS
- `pathpatterns`: `mode_t` is `u16` on macOS
- `proxmox-uuid`, `pxar` decoder: explicit reborrow for the
  `dangerous_implicit_autorefs` lint of newer Rust versions

## Updating

Copy the crates from the new upstream commit, update the table above and
reapply the changes listed here (`git diff` against the previous vendored
version shows them).
