//! Container lifecycle management.
//!
//! A [`ContainerManager`] turns a GitHub repository URL into a running
//! container and tears it back down again. The trait exists so the HTTP layer
//! can be written against it while the concrete backend (Docker, Firecracker,
//! a remote scheduler) is still undecided.

use std::fmt;

use crate::error::ApiError;

pub type ContainerResult<T> = Result<T, ContainerManagerError>;

/// An opaque handle to a container, assigned by whichever backend created it.
///
/// Callers should treat the inner string as meaningless and only pass it back
/// to the manager that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerId(String);

impl ContainerId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What can go wrong while managing a container.
///
/// This is deliberately independent of [`ApiError`] so the trait stays usable
/// from non-HTTP callers (a CLI, a background reconciler). The `From` impl
/// below is the single place that decides how each case surfaces to a client.
#[derive(Debug, thiserror::Error)]
pub enum ContainerManagerError {
    /// The URL was not something the manager can clone.
    #[error("`{0}` is not a valid GitHub repository URL")]
    InvalidRepositoryUrl(String),

    /// No container with this id exists, or it was already destroyed.
    #[error("container `{0}` does not exist")]
    NotFound(ContainerId),

    /// The backend refused because it is out of capacity.
    #[error("no capacity available to create a container")]
    CapacityExhausted,

    /// The backend was reachable but failed the operation.
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl From<ContainerManagerError> for ApiError {
    fn from(error: ContainerManagerError) -> Self {
        match error {
            ContainerManagerError::InvalidRepositoryUrl(_) => {
                Self::UnprocessableEntity(error.to_string())
            }
            ContainerManagerError::NotFound(_) => Self::NotFound(error.to_string()),
            // Retryable from the caller's point of view, but nothing about the
            // request itself was wrong.
            ContainerManagerError::CapacityExhausted => Self::Internal(error.into()),
            ContainerManagerError::Backend(source) => Self::Internal(source),
        }
    }
}

/// Creates and destroys containers for a given repository.
///
/// `#[async_trait]` boxes the returned futures so the trait stays
/// dyn-compatible: implementations are held as `Box<dyn ContainerManager>`
/// (or `Arc<..>`) and chosen at runtime.
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait ContainerManager: Send + Sync + 'static {
    /// Provisions a container for the repository at `github_url`, returning the
    /// id of the container that was created.
    async fn create_container(&self, github_url: &str) -> ContainerResult<ContainerId>;

    /// Tears down `container_id`, returning the id that was destroyed.
    async fn destroy_container(&self, container_id: &ContainerId) -> ContainerResult<ContainerId>;
}
