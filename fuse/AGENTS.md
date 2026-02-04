# fuse KNOWLEDGE BASE

FUSE adapter exposing FS9 as a POSIX filesystem. Bridges `fuser::Filesystem` trait to `Fs9Client` HTTP calls. Enables standard tools (git, vim, grep) on FS9.

## STRUCTURE

```
fuse/src/
├── main.rs     # CLI entry (sync main, manual tokio runtime), clap args, FUSE session setup
├── fs.rs       # Fs9Fuse: implements fuser::Filesystem (686 lines) — all FUSE ops
├── inode.rs    # InodeTable: bidirectional path ↔ inode mapping
├── handle.rs   # HandleTable: FUSE fh → FS9 Handle mapping
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| FUSE op behavior | `fs.rs` | Each method = one FUSE operation (lookup, getattr, read, write, etc.) |
| Inode allocation | `inode.rs` | Monotonic u64, path↔inode bidirectional map |
| File handle mapping | `handle.rs` | FUSE `fh` ↔ FS9 `Handle` translation |
| Mount options | `main.rs` | `--allow-other`, `--read-only`, `--cache-ttl`, `--auto-unmount` |
| Cache TTL tuning | `main.rs` + `fs.rs` | `cache_ttl` Duration passed to `Fs9Fuse::new()`, used in `getattr`/`lookup` |

## CONVENTIONS

- **Sync `main()`** — creates `tokio::runtime::Builder::new_multi_thread()` manually; NOT `#[tokio::main]`
- **All FS9 calls are `block_on()`** — FUSE callbacks are sync, bridge to async via `rt_handle.block_on()`
- **uid/gid from host** — uses `libc::getuid()/getgid()` (only `unsafe` in this crate)
- **Client-side, not core** — depends on `clients/rust` (Fs9Client), NOT on `core/` directly
- **Integration tests require running server** — all tests `#[ignore]`d, run with `--ignored` flag

## ANTI-PATTERNS

- **Don't use `#[tokio::main]`** — FUSE session must own the thread; tokio runtime is a side-channel
- **Don't skip inode table** — every path must have a stable inode for FUSE consistency
- **Don't test without server** — FUSE tests are integration-only, marked `#[ignore]`

## NOTES

- 1114-line integration test file (`tests/integration.rs`) covers Git workflows, bash pipes, and standard tool compatibility
- Ctrl-C handler triggers clean unmount via `session.unmount_callable()`
- Config merges CLI args → YAML config → defaults (CLI wins)
