//! Builder pattern for creating and mounting FS9 FUSE filesystems.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fs9_client::Fs9Client;
use fuser::MountOption;
use tokio::runtime::Handle as TokioHandle;

use crate::fs::Fs9Fuse;

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub struct MountOptions {
    pub allow_other: bool,
    pub allow_root: bool,
    pub auto_unmount: bool,
    pub read_only: bool,
}

impl MountOptions {
    /// Convert to a list of [`fuser::MountOption`] values.
    ///
    /// The base options (`FSName`, `Subtype`, `DefaultPermissions`) are always
    /// included. Boolean flags add their corresponding FUSE option when `true`.
    #[must_use]
    pub fn to_fuser_options(&self) -> Vec<MountOption> {
        let mut options = vec![
            MountOption::FSName("fs9".to_string()),
            MountOption::Subtype("fs9".to_string()),
            MountOption::DefaultPermissions,
        ];
        if self.allow_other {
            options.push(MountOption::AllowOther);
        }
        if self.allow_root {
            options.push(MountOption::AllowRoot);
        }
        if self.auto_unmount {
            options.push(MountOption::AutoUnmount);
        }
        if self.read_only {
            options.push(MountOption::RO);
        }
        options
    }
}

/// Builder for creating and mounting FS9 FUSE filesystems.
///
/// # Example
///
/// ```no_run
/// use fs9_fuse::Fs9FuseBuilder;
/// use fs9_client::Fs9Client;
/// use std::time::Duration;
/// use std::path::Path;
///
/// let client = Fs9Client::builder("http://localhost:9999").build().unwrap();
/// let rt = tokio::runtime::Handle::current();
/// let mount = Fs9FuseBuilder::new(client, rt)
///     .cache_ttl(Duration::from_secs(5))
///     .allow_other(true)
///     .mount(Path::new("/mnt/fs9"))
///     .unwrap();
/// ```
pub struct Fs9FuseBuilder {
    client: Fs9Client,
    runtime: TokioHandle,
    uid: u32,
    gid: u32,
    cache_ttl: Duration,
    mount_options: MountOptions,
}

impl Fs9FuseBuilder {
    /// Create a new builder with the required parameters.
    ///
    /// `uid` and `gid` default to the current process's effective uid/gid.
    /// `cache_ttl` defaults to 1 second.
    #[must_use]
    #[allow(unsafe_code)]
    pub fn new(client: Fs9Client, runtime: TokioHandle) -> Self {
        // SAFETY: getuid/getgid are always safe to call — they have no failure mode.
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        Self {
            client,
            runtime,
            uid,
            gid,
            cache_ttl: Duration::from_secs(1),
            mount_options: MountOptions::default(),
        }
    }

    #[must_use]
    pub const fn uid(mut self, uid: u32) -> Self {
        self.uid = uid;
        self
    }

    #[must_use]
    pub const fn gid(mut self, gid: u32) -> Self {
        self.gid = gid;
        self
    }

    #[must_use]
    pub const fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    #[must_use]
    pub const fn allow_other(mut self, allow: bool) -> Self {
        self.mount_options.allow_other = allow;
        self
    }

    #[must_use]
    pub const fn allow_root(mut self, allow: bool) -> Self {
        self.mount_options.allow_root = allow;
        self
    }

    #[must_use]
    pub const fn auto_unmount(mut self, auto: bool) -> Self {
        self.mount_options.auto_unmount = auto;
        self
    }

    #[must_use]
    pub const fn read_only(mut self, ro: bool) -> Self {
        self.mount_options.read_only = ro;
        self
    }

    /// Build the [`Fs9Fuse`] instance and mount options without mounting.
    ///
    /// For advanced use cases where you want to control the FUSE session
    /// yourself (e.g. foreground mode with custom signal handling).
    #[must_use]
    pub fn build(self) -> (Fs9Fuse, Vec<MountOption>) {
        let fs = Fs9Fuse::new(
            self.client,
            self.runtime,
            self.uid,
            self.gid,
            self.cache_ttl,
        );
        let options = self.mount_options.to_fuser_options();
        (fs, options)
    }

    /// Mount the filesystem at the given path and return an RAII guard.
    ///
    /// The returned [`Fs9FuseMount`] runs the FUSE session in a background
    /// thread.  The filesystem is unmounted when the guard is dropped.
    /// Call [`.join()`](Fs9FuseMount::join) to block until external unmount.
    ///
    /// # Errors
    ///
    /// Returns an error if the FUSE session cannot be created or the
    /// background thread fails to spawn.
    pub fn mount(self, mountpoint: &Path) -> Result<Fs9FuseMount, std::io::Error> {
        let mountpoint = mountpoint.to_path_buf();
        let (fs, options) = self.build();
        let session = fuser::Session::new(fs, &mountpoint, &options)?;
        let guard = session.spawn()?;
        Ok(Fs9FuseMount { mountpoint, guard })
    }
}

/// RAII guard for a mounted FUSE filesystem.
///
/// When this value is dropped, the FUSE filesystem is unmounted.
pub struct Fs9FuseMount {
    mountpoint: PathBuf,
    guard: fuser::BackgroundSession,
}

impl Fs9FuseMount {
    #[must_use]
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    pub fn join(self) {
        self.guard.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mount_options_default() {
        let opts = MountOptions::default();
        assert!(!opts.allow_other);
        assert!(!opts.allow_root);
        assert!(!opts.auto_unmount);
        assert!(!opts.read_only);
    }

    #[test]
    fn test_mount_options_to_fuser_base() {
        let opts = MountOptions::default();
        let fuser_opts = opts.to_fuser_options();
        // Base options: FSName, Subtype, DefaultPermissions
        assert_eq!(fuser_opts.len(), 3);
    }

    #[test]
    fn test_mount_options_to_fuser_all() {
        let opts = MountOptions {
            allow_other: true,
            allow_root: true,
            auto_unmount: true,
            read_only: true,
        };
        let fuser_opts = opts.to_fuser_options();
        // Base (3) + AllowOther + AllowRoot + AutoUnmount + RO = 7
        assert_eq!(fuser_opts.len(), 7);
    }

    #[test]
    fn test_builder_setters() {
        // We can't fully test without a real client, but we can verify
        // the builder compiles and the const setters work
        // This test just verifies the API compiles correctly
        let _: fn(Fs9FuseBuilder) -> Fs9FuseBuilder = |b| {
            b.uid(1000)
                .gid(1000)
                .cache_ttl(Duration::from_secs(5))
                .allow_other(true)
                .allow_root(false)
                .auto_unmount(true)
                .read_only(false)
        };
    }
}
