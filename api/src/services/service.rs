//! Service lifecycle management.
//!
//! A [`ServiceManager`] turns a GitHub repository URL into a running service
//! and tears it back down again. "Service" is the deployed-workload sense the
//! hosting backends use, not the `crate::services` sense of a capability
//! trait. The trait exists so the HTTP layer can be written against it while
//! the concrete backend (Docker, Firecracker, a remote scheduler) is still
//! undecided.

use std::fmt;

use crate::error::ApiError;

pub type ServiceResult<T> = Result<T, ServiceManagerError>;

/// An opaque handle to a service, assigned by whichever backend created it.
///
/// Callers should treat the inner string as meaningless and only pass it back
/// to the manager that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceId(String);

impl ServiceId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What can go wrong while managing a service.
///
/// This is deliberately independent of [`ApiError`] so the trait stays usable
/// from non-HTTP callers (a CLI, a background reconciler). The `From` impl
/// below is the single place that decides how each case surfaces to a client.
#[derive(Debug, thiserror::Error)]
pub enum ServiceManagerError {
    /// The URL was not something the manager can clone.
    #[error("`{0}` is not a valid GitHub repository URL")]
    InvalidRepositoryUrl(String),

    /// No service with this id exists, or it was already destroyed.
    #[error("service `{0}` does not exist")]
    NotFound(ServiceId),

    /// The backend refused because it is out of capacity.
    #[error("no capacity available to create a service")]
    CapacityExhausted,

    /// The backend was reachable but failed the operation.
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl From<ServiceManagerError> for ApiError {
    fn from(error: ServiceManagerError) -> Self {
        match error {
            ServiceManagerError::InvalidRepositoryUrl(_) => {
                Self::UnprocessableEntity(error.to_string())
            }
            ServiceManagerError::NotFound(_) => Self::NotFound(error.to_string()),
            // Retryable from the caller's point of view, but nothing about the
            // request itself was wrong.
            ServiceManagerError::CapacityExhausted => Self::Internal(error.into()),
            ServiceManagerError::Backend(source) => Self::Internal(source),
        }
    }
}

/// Creates and destroys services for a given repository.
///
/// `#[async_trait]` boxes the returned futures so the trait stays
/// dyn-compatible: implementations are held as `Box<dyn ServiceManager>`
/// (or `Arc<..>`) and chosen at runtime.
#[async_trait::async_trait]
pub trait ServiceManager: Send + Sync + 'static {
    /// Provisions a service for the repository at `github_url`, returning the
    /// id of the service that was created.
    async fn create_service(&self, github_url: &str) -> ServiceResult<ServiceId>;

    /// Tears down `service_id`, returning the id that was destroyed.
    async fn destroy_service(&self, service_id: &ServiceId) -> ServiceResult<ServiceId>;
}
