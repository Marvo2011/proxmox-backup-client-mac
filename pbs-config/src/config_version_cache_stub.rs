//! Stand-in for the shared memory config version cache on platforms without
//! robust process-shared mutexes (only the client is built there, which never
//! touches the server configuration).

use std::sync::Arc;

use anyhow::{bail, Error};

pub struct ConfigVersionCache {
    _private: (),
}

impl ConfigVersionCache {
    pub fn new() -> Result<Arc<Self>, Error> {
        bail!("config version cache is not supported on this platform");
    }

    pub fn user_cache_generation(&self) -> usize {
        unreachable!()
    }

    pub fn increase_user_cache_generation(&self) {
        unreachable!()
    }

    pub fn traffic_control_generation(&self) -> usize {
        unreachable!()
    }

    pub fn increase_traffic_control_generation(&self) {
        unreachable!()
    }

    pub fn increase_datastore_generation(&self) -> usize {
        unreachable!()
    }
}
