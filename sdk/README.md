# fs9-sdk

Core SDK for FS9 distributed file system. Provides the `FsProvider` trait and supporting types for building storage backends.

## Usage

```rust,ignore
use fs9_sdk::{FsProvider, Capabilities, FsError, FileInfo, Handle, OpenFlags};
use async_trait::async_trait;
use bytes::Bytes;

struct MyProvider;

#[async_trait]
impl FsProvider for MyProvider {
    // Implement the 10 methods...
}
```

## Core Types

- `FsProvider` - The 10-method trait all storage backends implement
- `FileInfo` - File/directory metadata
- `FileType` - Regular, Directory, or Symlink
- `StatChanges` - Metadata modification request (Plan 9 wstat style)
- `FsStats` - Filesystem statistics
- `OpenFlags` - File open flags
- `Handle` - Opaque file handle
- `Capabilities` - Bitflags describing backend capabilities
- `FsError` - Error type with HTTP status mapping
