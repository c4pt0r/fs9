# FS9 Performance Optimization Plan

**Target**: Support 1M+ users with 100 ops/s throughput
**Created**: 2026-02-05
**Status**: In Progress

## Executive Summary

This plan addresses critical performance bottlenecks identified in the FS9 distributed filesystem to achieve production-grade scalability. Optimizations are prioritized by impact and grouped into phases.

---

## Phase 1: Critical Path Optimizations (P0)

### 1.1 HandleRegistry Sharded Lock

**Problem**: Global `RwLock<HashMap>` creates contention under high concurrency. Even read locks cause cache-line invalidation across cores.

**Current Code** (`core/src/handle.rs:32-36`):
```rust
pub struct HandleRegistry {
    handles: RwLock<HashMap<HandleId, HandleState>>,
    next_id: AtomicU64,
    ttl: Duration,
}
```

**Solution**: Implement sharded locking with 64 independent shards.

**Implementation**:
```rust
const NUM_SHARDS: usize = 64;

pub struct HandleRegistry {
    shards: [RwLock<HashMap<HandleId, HandleState>>; NUM_SHARDS],
    next_id: AtomicU64,
    ttl: Duration,
}

impl HandleRegistry {
    fn shard_for(&self, id: HandleId) -> &RwLock<HashMap<HandleId, HandleState>> {
        &self.shards[(id as usize) % NUM_SHARDS]
    }
}
```

**Expected Impact**: 5-10x improvement in read throughput under contention.

**Files to Modify**:
- `core/src/handle.rs`

**Tests to Add**:
- Concurrent handle registration stress test
- Concurrent read/write operations test
- Shard distribution uniformity test

---

### 1.2 Plugin FFI Async Safety

**Problem**: Synchronous FFI calls execute on async runtime threads, blocking the tokio worker pool. If plugins perform any I/O (e.g., S3 backend), this starves other tasks.

**Current Code** (`core/src/plugin.rs:450-467`):
```rust
async fn stat(&self, path: &str) -> FsResult<FileInfo> {
    let result = unsafe {
        (self.plugin.vtable.stat)(self.provider, ...)  // Blocking!
    };
}
```

**Solution**: Offload FFI calls to `spawn_blocking` thread pool.

**Implementation**:
```rust
async fn stat(&self, path: &str) -> FsResult<FileInfo> {
    let plugin = self.plugin.clone();
    let provider = self.provider;
    let path = path.to_string();
    
    tokio::task::spawn_blocking(move || {
        // FFI call here
    }).await.map_err(|e| FsError::internal(e.to_string()))?
}
```

**Expected Impact**: Prevents runtime thread starvation; maintains async throughput.

**Files to Modify**:
- `core/src/plugin.rs` (all FsProvider trait methods)

**Tests to Add**:
- Concurrent plugin operations under load
- Verify no runtime blocking with slow plugin

---

## Phase 2: High-Impact Optimizations (P1)

### 2.1 MountTable O(log n) Path Resolution

**Problem**: Current implementation iterates all mount points O(n) despite using BTreeMap.

**Current Code** (`core/src/mount.rs:101-111`):
```rust
for (mount_path, entry) in mounts.iter() {  // O(n) iteration
    if path == *mount_path || path.starts_with(...) {
        // ...
    }
}
```

**Solution**: Leverage BTreeMap ordering with reverse range iteration.

**Implementation**:
```rust
pub async fn resolve(&self, path: &str) -> FsResult<(Arc<dyn FsProvider>, String)> {
    let mounts = self.mounts.read().await;
    
    // Find longest matching prefix using reverse iteration from path
    for (mount_path, entry) in mounts.range(..=path.to_string()).rev() {
        if path == mount_path || path.starts_with(&format!("{mount_path}/")) {
            return Ok((entry.provider.clone(), self.compute_relative(path, mount_path)));
        }
        // Early termination when no longer a potential prefix
        if !self.could_be_prefix(mount_path, path) {
            break;
        }
    }
    
    // Check root mount
    if let Some(entry) = mounts.get("/") {
        return Ok((entry.provider.clone(), path.to_string()));
    }
    
    Err(FsError::not_found(path))
}
```

**Expected Impact**: O(n) → O(log n) path resolution.

**Files to Modify**:
- `core/src/mount.rs`

**Tests to Add**:
- Benchmark with 100+ mount points
- Edge cases: root mount, exact match, deep nesting

---

### 2.2 TokenCache with Capacity Limit

**Problem**: Unbounded HashMap grows with user count. 1M users × 200 bytes/token = 200MB+ memory.

**Current Code** (`server/src/token_cache.rs:31-34`):
```rust
pub struct TokenCache {
    cache: Arc<RwLock<HashMap<String, CachedToken>>>,
    ttl: Duration,
}
```

**Solution**: Use `moka` crate for bounded, concurrent cache with TTL.

**Implementation**:
```rust
use moka::future::Cache;

pub struct TokenCache {
    cache: Cache<String, CachedToken>,
}

impl TokenCache {
    pub fn new(ttl: Duration, max_capacity: u64) -> Self {
        Self {
            cache: Cache::builder()
                .max_capacity(max_capacity)
                .time_to_live(ttl)
                .build(),
        }
    }
}
```

**Expected Impact**: Bounded memory usage; better concurrent performance.

**Files to Modify**:
- `server/src/token_cache.rs`
- `server/Cargo.toml` (add moka dependency)

**Tests to Add**:
- Capacity eviction test
- Concurrent access stress test

---

### 2.3 Handle Cleanup Background Task

**Problem**: `cleanup_stale()` must be called manually. Leaked handles cause memory growth.

**Solution**: Start background task on server initialization.

**Implementation**:
```rust
// core/src/handle.rs
pub fn start_cleanup_task(
    registry: Arc<HandleRegistry>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let closed = registry.cleanup_stale().await;
            if !closed.is_empty() {
                tracing::debug!(count = closed.len(), "Cleaned stale handles");
            }
        }
    })
}
```

**Files to Modify**:
- `core/src/handle.rs`
- `server/src/main.rs` (start task)

**Tests to Add**:
- Verify stale handles are cleaned automatically

---

## Phase 3: Stability Optimizations (P2)

### 3.1 VfsRouter::open Avoid Double Stat

**Problem**: `open()` calls `stat()` immediately after, but provider already has metadata.

**Solution**: Modify `FsProvider::open` to optionally return metadata, or cache stat result.

**Implementation** (minimal change approach):
```rust
// In VfsRouter::open, reuse info if provider supports it
async fn open(&self, path: &str, flags: OpenFlags) -> FsResult<Handle> {
    let (provider, relative_path) = self.resolve(path).await?;
    // ...
    let provider_handle = provider.open(&relative_path, flags).await?;
    
    // For providers that cache open file info, avoid re-stat
    let metadata = if flags.create {
        // Must stat after create to get accurate info
        provider.stat(&relative_path).await?
    } else {
        // Try to get from provider's cache or stat
        provider.stat(&relative_path).await?
    };
    // ...
}
```

**Note**: Full optimization requires `FsProvider` trait change (breaking). Consider for v2.

**Files to Modify**:
- `core/src/vfs.rs`
- Potentially `sdk/src/provider.rs` (trait extension)

---

### 3.2 Request Timeout and Backpressure

**Problem**: No request-level timeout or rate limiting. Slow requests can starve the system.

**Solution**: Add tower middleware layers.

**Implementation**:
```rust
use tower::timeout::TimeoutLayer;
use tower::limit::ConcurrencyLimitLayer;

let app = api::create_router(state)
    .layer(middleware::from_fn_with_state(...))
    .layer(TimeoutLayer::new(Duration::from_secs(30)))
    .layer(ConcurrencyLimitLayer::new(1000))  // Max concurrent requests
    .layer(TraceLayer::new_for_http());
```

**Files to Modify**:
- `server/src/main.rs`
- `server/Cargo.toml` (tower features)

**Tests to Add**:
- Timeout behavior test
- Concurrency limit test

---

## Phase 4: Polish Optimizations (P3)

### 4.1 NamespaceManager Lock Optimization

**Problem**: Write lock held during namespace creation blocks all reads.

**Solution**: Double-checked locking with pre-creation outside lock.

**Files to Modify**:
- `server/src/namespace.rs`

---

### 4.2 HandleMap Compact Representation

**Problem**: UUID strings (36 bytes) stored bidirectionally. Memory inefficient.

**Solution**: Use base64-encoded u64 or expose handle ID directly.

**Files to Modify**:
- `server/src/state.rs`
- `server/src/api/handlers.rs`

---

## Dependencies to Add

```toml
# server/Cargo.toml
[dependencies]
moka = { version = "0.12", features = ["future"] }
tower = { version = "0.4", features = ["timeout", "limit"] }
```

---

## Testing Strategy

### Unit Tests
- Each optimization has dedicated unit tests
- Maintain existing test coverage

### Integration Tests
- Concurrent operation stress tests
- Memory usage under load
- Latency percentile measurements

### Benchmarks
- Before/after comparison for each optimization
- Use `criterion` for micro-benchmarks

---

## Rollout Plan

1. **Phase 1 (P0)**: Deploy immediately after testing
2. **Phase 2 (P1)**: Deploy within 1 week
3. **Phase 3 (P2)**: Deploy within 2 weeks
4. **Phase 4 (P3)**: Deploy as time permits

---

## Success Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Handle ops throughput | ~1K ops/s | 10K+ ops/s |
| Path resolution latency (p99) | Unknown | <1ms |
| Memory per 100K tokens | ~20MB | <10MB |
| Max concurrent handles | ~10K | 100K+ |

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Sharded lock increases complexity | Comprehensive tests, clear documentation |
| spawn_blocking pool exhaustion | Configure pool size, monitor queue depth |
| moka dependency adds bloat | Evaluate size impact, consider alternatives |
| Breaking trait changes | Defer to v2, use extension traits |
