//! Type-keyed state that API handlers can request as a parameter.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use anyhow::{Error, bail};

/// Registry of state values, keyed by their type.
///
/// It holds at most one value per type. Values are registered with
/// [`register`](SharedStateRegistry::register) before the registry is handed over to the API
/// environment, from where handlers can access them through
/// [`RpcEnvironment::shared_state`](crate::RpcEnvironment::shared_state). This allows passing
/// application context to handlers without resorting to globals.
#[derive(Default)]
pub struct SharedStateRegistry {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl SharedStateRegistry {
    /// Return a clone of the registered value of type `T`, if there is one.
    pub fn lookup<T: 'static + Send + Sync + Clone>(&self) -> Option<T> {
        self.map.get(&TypeId::of::<T>()).map(|s| {
            s.downcast_ref()
                .cloned()
                .expect("mismatch between typeid and contained type")
        })
    }

    /// Register a value so that handlers can request it in a `#[state]` parameter.
    ///
    /// Since the type is the key, wrap values in a newtype if their type alone is not specific
    /// enough to identify them.
    ///
    /// Fails if a value of type `T` was already registered.
    pub fn register<T: 'static + Send + Sync + Clone>(&mut self, data: T) -> Result<(), Error> {
        let type_id = TypeId::of::<T>();

        if self.map.contains_key(&type_id) {
            bail!("type already registered");
        }

        self.map.insert(type_id, Box::new(data));

        Ok(())
    }
}
