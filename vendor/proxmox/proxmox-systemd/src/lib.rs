//! Systemd communication.

// libsystemd is only available on Linux, elsewhere only the unit name escaping is provided
#[cfg(target_os = "linux")]
pub(crate) mod sys;

mod escape;
pub use escape::{UnescapeError, escape_unit, unescape_unit, unescape_unit_path};

#[cfg(target_os = "linux")]
pub mod journal;
#[cfg(target_os = "linux")]
pub mod notify;

#[cfg(target_os = "linux")]
pub mod sd_id128;
