mod types;
mod error;
mod client;

pub use client::Fs9Client;
pub use error::{Fs9Error, Result};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builder() {
        let client = Fs9Client::builder("http://localhost:8080")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[test]
    fn open_flags_constructors() {
        let read = OpenFlags::read();
        assert!(read.read);
        assert!(!read.write);

        let write = OpenFlags::write();
        assert!(!write.read);
        assert!(write.write);

        let create = OpenFlags::create();
        assert!(create.read);
        assert!(create.write);
        assert!(create.create);
    }

    #[test]
    fn stat_changes_builder() {
        let changes = StatChanges::new()
            .mode(0o644)
            .size(100);
        assert_eq!(changes.mode, Some(0o644));
        assert_eq!(changes.size, Some(100));
        assert!(changes.name.is_none());
    }
}
