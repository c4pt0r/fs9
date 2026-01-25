use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Capabilities: u64 {
        const READ          = 1 << 0;
        const WRITE         = 1 << 1;
        const CREATE        = 1 << 2;
        const DELETE        = 1 << 3;
        const RENAME        = 1 << 4;
        const TRUNCATE      = 1 << 5;

        const CHMOD         = 1 << 10;
        const CHOWN         = 1 << 11;
        const UTIME         = 1 << 12;

        const HARDLINK      = 1 << 20;
        const SYMLINK       = 1 << 21;

        const SYNC          = 1 << 30;
        const APPEND        = 1 << 31;
        const RANDOM_WRITE  = 1 << 32;
        const STREAMING     = 1 << 33;
        const BLOCKING_READ = 1 << 34;

        const VERSIONING    = 1 << 40;
        const ETAG          = 1 << 41;
        const ATOMIC_RENAME = 1 << 42;
        const DIRECTORY     = 1 << 43;
        const XATTR         = 1 << 44;

        const SYNTHETIC     = 1 << 50;
        const STATEFUL_READ = 1 << 51;
        const STATEFUL_WRITE = 1 << 52;
    }
}

impl Capabilities {
    pub const READONLY: Self = Self::READ.union(Self::DIRECTORY);

    pub const BASIC_RW: Self = Self::READ
        .union(Self::WRITE)
        .union(Self::CREATE)
        .union(Self::DELETE)
        .union(Self::DIRECTORY);

    pub const POSIX_LIKE: Self = Self::BASIC_RW
        .union(Self::RENAME)
        .union(Self::TRUNCATE)
        .union(Self::CHMOD)
        .union(Self::CHOWN)
        .union(Self::UTIME)
        .union(Self::SYMLINK)
        .union(Self::XATTR)
        .union(Self::SYNC)
        .union(Self::RANDOM_WRITE);

    pub const QUEUE_LIKE: Self = Self::READ
        .union(Self::WRITE)
        .union(Self::CREATE)
        .union(Self::DELETE)
        .union(Self::DIRECTORY)
        .union(Self::SYNTHETIC)
        .union(Self::STATEFUL_READ)
        .union(Self::STATEFUL_WRITE)
        .union(Self::BLOCKING_READ);

    #[must_use]
    pub fn supports_read(&self) -> bool {
        self.contains(Self::READ)
    }

    #[must_use]
    pub fn supports_write(&self) -> bool {
        self.contains(Self::WRITE)
    }

    #[must_use]
    pub fn supports_create(&self) -> bool {
        self.contains(Self::CREATE)
    }

    #[must_use]
    pub fn supports_delete(&self) -> bool {
        self.contains(Self::DELETE)
    }

    #[must_use]
    pub fn supports_rename(&self) -> bool {
        self.contains(Self::RENAME)
    }

    #[must_use]
    pub fn supports_truncate(&self) -> bool {
        self.contains(Self::TRUNCATE)
    }

    #[must_use]
    pub fn supports_chmod(&self) -> bool {
        self.contains(Self::CHMOD)
    }

    #[must_use]
    pub fn supports_chown(&self) -> bool {
        self.contains(Self::CHOWN)
    }

    #[must_use]
    pub fn supports_symlink(&self) -> bool {
        self.contains(Self::SYMLINK)
    }

    #[must_use]
    pub fn supports_directories(&self) -> bool {
        self.contains(Self::DIRECTORY)
    }

    #[must_use]
    pub fn supports_random_write(&self) -> bool {
        self.contains(Self::RANDOM_WRITE)
    }

    #[must_use]
    pub fn is_readonly(&self) -> bool {
        !self.contains(Self::WRITE) && !self.contains(Self::CREATE) && !self.contains(Self::DELETE)
    }

    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        self.contains(Self::SYNTHETIC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_readonly() {
        let caps = Capabilities::READONLY;
        assert!(caps.supports_read());
        assert!(caps.supports_directories());
        assert!(!caps.supports_write());
        assert!(!caps.supports_create());
        assert!(caps.is_readonly());
    }

    #[test]
    fn preset_basic_rw() {
        let caps = Capabilities::BASIC_RW;
        assert!(caps.supports_read());
        assert!(caps.supports_write());
        assert!(caps.supports_create());
        assert!(caps.supports_delete());
        assert!(caps.supports_directories());
        assert!(!caps.supports_rename());
        assert!(!caps.supports_chmod());
        assert!(!caps.is_readonly());
    }

    #[test]
    fn preset_posix_like() {
        let caps = Capabilities::POSIX_LIKE;
        assert!(caps.supports_read());
        assert!(caps.supports_write());
        assert!(caps.supports_create());
        assert!(caps.supports_delete());
        assert!(caps.supports_rename());
        assert!(caps.supports_truncate());
        assert!(caps.supports_chmod());
        assert!(caps.supports_chown());
        assert!(caps.supports_symlink());
        assert!(caps.supports_random_write());
        assert!(caps.contains(Capabilities::SYNC));
        assert!(caps.contains(Capabilities::XATTR));
    }

    #[test]
    fn preset_queue_like() {
        let caps = Capabilities::QUEUE_LIKE;
        assert!(caps.supports_read());
        assert!(caps.supports_write());
        assert!(caps.is_synthetic());
        assert!(caps.contains(Capabilities::STATEFUL_READ));
        assert!(caps.contains(Capabilities::STATEFUL_WRITE));
        assert!(caps.contains(Capabilities::BLOCKING_READ));
        assert!(!caps.supports_random_write());
    }

    #[test]
    fn custom_capabilities() {
        let caps =
            Capabilities::READ | Capabilities::WRITE | Capabilities::ETAG | Capabilities::STREAMING;
        assert!(caps.supports_read());
        assert!(caps.supports_write());
        assert!(caps.contains(Capabilities::ETAG));
        assert!(caps.contains(Capabilities::STREAMING));
        assert!(!caps.supports_directories());
    }

    #[test]
    fn capability_combination() {
        let base = Capabilities::BASIC_RW;
        let extended = base | Capabilities::ETAG | Capabilities::ATOMIC_RENAME;
        assert!(extended.contains(Capabilities::ETAG));
        assert!(extended.contains(Capabilities::ATOMIC_RENAME));
        assert!(extended.supports_read());
    }
}
