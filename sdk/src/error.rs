use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum FsError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not a directory: {0}")]
    NotDirectory(String),

    #[error("is a directory: {0}")]
    IsDirectory(String),

    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),

    #[error("invalid handle: {0}")]
    InvalidHandle(u64),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("storage backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("transient error (retryable): {0}")]
    Transient(String),

    #[error("remote error from {node}: {message}")]
    Remote { node: String, message: String },

    #[error("timeout after {duration:?}")]
    Timeout { duration: Duration },

    #[error("circuit breaker open for {service}")]
    CircuitBreakerOpen { service: String },

    #[error("too many proxy hops: {depth} (max: {max})")]
    TooManyHops { depth: usize, max: usize },

    #[error("conflict: ETag mismatch (expected {expected}, got {actual})")]
    Conflict { expected: String, actual: String },

    #[error("version conflict: expected {expected}, got {actual}")]
    VersionConflict { expected: u64, actual: u64 },
}

impl FsError {
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transient(_) | Self::Timeout { .. } | Self::BackendUnavailable(_)
        )
    }

    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::PermissionDenied(_))
    }

    #[must_use]
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict { .. } | Self::VersionConflict { .. })
    }

    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::PermissionDenied(_) => 403,
            Self::AlreadyExists(_) | Self::Conflict { .. } | Self::VersionConflict { .. } => 409,
            Self::InvalidArgument(_) | Self::InvalidHandle(_) => 400,
            Self::NotDirectory(_) | Self::IsDirectory(_) | Self::DirectoryNotEmpty(_) => 400,
            Self::NotImplemented(_) => 501,
            Self::Transient(_) | Self::BackendUnavailable(_) | Self::CircuitBreakerOpen { .. } => {
                503
            }
            Self::Timeout { .. } => 504,
            Self::TooManyHops { .. } => 508,
            Self::Internal(_) | Self::Remote { .. } => 500,
        }
    }

    #[must_use]
    pub fn not_found(path: impl Into<String>) -> Self {
        Self::NotFound(path.into())
    }

    #[must_use]
    pub fn permission_denied(reason: impl Into<String>) -> Self {
        Self::PermissionDenied(reason.into())
    }

    #[must_use]
    pub fn already_exists(path: impl Into<String>) -> Self {
        Self::AlreadyExists(path.into())
    }

    #[must_use]
    pub fn invalid_argument(reason: impl Into<String>) -> Self {
        Self::InvalidArgument(reason.into())
    }

    #[must_use]
    pub fn not_directory(path: impl Into<String>) -> Self {
        Self::NotDirectory(path.into())
    }

    #[must_use]
    pub fn is_directory(path: impl Into<String>) -> Self {
        Self::IsDirectory(path.into())
    }

    #[must_use]
    pub fn directory_not_empty(path: impl Into<String>) -> Self {
        Self::DirectoryNotEmpty(path.into())
    }

    #[must_use]
    pub fn invalid_handle(id: u64) -> Self {
        Self::InvalidHandle(id)
    }

    #[must_use]
    pub fn internal(reason: impl Into<String>) -> Self {
        Self::Internal(reason.into())
    }

    #[must_use]
    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self::NotImplemented(feature.into())
    }

    #[must_use]
    pub fn backend_unavailable(reason: impl Into<String>) -> Self {
        Self::BackendUnavailable(reason.into())
    }

    #[must_use]
    pub fn transient(reason: impl Into<String>) -> Self {
        Self::Transient(reason.into())
    }

    #[must_use]
    pub fn timeout(duration: Duration) -> Self {
        Self::Timeout { duration }
    }
}

pub type FsResult<T> = Result<T, FsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_errors() {
        assert!(FsError::transient("network error").is_retryable());
        assert!(FsError::timeout(Duration::from_secs(30)).is_retryable());
        assert!(FsError::backend_unavailable("s3").is_retryable());

        assert!(!FsError::not_found("/path").is_retryable());
        assert!(!FsError::permission_denied("access").is_retryable());
        assert!(!FsError::invalid_argument("bad").is_retryable());
    }

    #[test]
    fn http_status_codes() {
        assert_eq!(FsError::not_found("/path").http_status(), 404);
        assert_eq!(FsError::permission_denied("access").http_status(), 403);
        assert_eq!(FsError::already_exists("/path").http_status(), 409);
        assert_eq!(FsError::invalid_argument("bad").http_status(), 400);
        assert_eq!(FsError::not_implemented("feature").http_status(), 501);
        assert_eq!(FsError::transient("error").http_status(), 503);
        assert_eq!(FsError::timeout(Duration::from_secs(30)).http_status(), 504);
        assert_eq!(
            FsError::TooManyHops { depth: 10, max: 8 }.http_status(),
            508
        );
        assert_eq!(FsError::internal("error").http_status(), 500);
    }

    #[test]
    fn error_predicates() {
        assert!(FsError::not_found("/path").is_not_found());
        assert!(!FsError::permission_denied("access").is_not_found());

        assert!(FsError::permission_denied("access").is_permission_denied());
        assert!(!FsError::not_found("/path").is_permission_denied());

        assert!(FsError::Conflict {
            expected: "a".into(),
            actual: "b".into()
        }
        .is_conflict());
        assert!(FsError::VersionConflict {
            expected: 1,
            actual: 2
        }
        .is_conflict());
        assert!(!FsError::not_found("/path").is_conflict());
    }

    #[test]
    fn error_display() {
        let err = FsError::not_found("/test/file.txt");
        assert_eq!(err.to_string(), "not found: /test/file.txt");

        let err = FsError::TooManyHops { depth: 10, max: 8 };
        assert_eq!(err.to_string(), "too many proxy hops: 10 (max: 8)");
    }
}
