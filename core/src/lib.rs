#![allow(missing_docs)]

pub mod handle;
pub mod mount;
pub mod plugin;
pub mod providers;
pub mod vfs;

pub use fs9_sdk;
pub use handle::{HandleId, HandleInfo, HandleRef, HandleRegistry, HandleState};
pub use mount::{MountEntry, MountPoint, MountTable};
pub use plugin::{PluginError, PluginManager, PluginProvider};
pub use providers::{
    default_registry, LocalFs, MemoryFs, ProxyFs, ProviderConfig, ProviderFactory,
    ProviderRegistry, StreamFS,
};
pub use vfs::VfsRouter;
