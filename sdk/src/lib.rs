#![doc = include_str!("../README.md")]

mod capabilities;
mod error;
mod provider;
mod types;

pub use capabilities::Capabilities;
pub use error::{FsError, FsResult};
pub use provider::FsProvider;
pub use types::{FileInfo, FileType, FsStats, Handle, OpenFlags, StatChanges};
