#[cfg(target_os = "linux")]
pub mod loopdev;

#[cfg(target_os = "macos")]
mod hdiutil;

mod fuse_loop;
pub use fuse_loop::*;
