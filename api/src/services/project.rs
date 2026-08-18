//! Project lifecycle management.
//!
//! A project is the service's owner: it groups related services and is what a
//! backend bills and scopes access against. [`ProjectManager`] is the CRUD
//! surface over that grouping, kept separate from
//! [`ServiceManager`](crate::services::service::ServiceManager) so a backend
//! can implement one without the other.

use std::fmt;

use crate::error::ApiError;

pub type ProjectResult<T> = Result<T, ProjectManagerError>;

/// An opaque handle to a project, assigned by whichever backend created it.
///
/// Callers should treat the inner string as meaningless and only pass it back
/// to the manager that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A project as the backend reports it.
///
/// Deliberately thin. Fields get added when a caller needs them rather than
/// mirroring whatever the backend happens to return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
}

/// What can go wrong while managing a project.
///
/// Independent of [`ApiError`] for the same reason as
/// [`ServiceManagerError`](crate::services::service::ServiceManagerError): the
/// trait stays usable from non-HTTP callers, and one `From` impl decides how
/// each case surfaces to a client.
#[derive(Debug, thiserror::Error)]
pub enum ProjectManagerError {
    /// The requested name is empty, too long, or otherwise unusable.
    #[error("`{0}` is not a valid project name")]
    InvalidName(String),

    /// No project with this id exists, or it was already deleted.
    #[error("project `{0}` does not exist")]
    NotFound(ProjectId),

    /// A project with this name already exists.
    #[error("a project named `{0}` already exists")]
    AlreadyExists(String),

    /// The backend was reachable but failed the operation.
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl From<ProjectManagerError> for ApiError {
    fn from(error: ProjectManagerError) -> Self {
        match error {
            ProjectManagerError::InvalidName(_) => Self::UnprocessableEntity(error.to_string()),
            ProjectManagerError::NotFound(_) => Self::NotFound(error.to_string()),
            ProjectManagerError::AlreadyExists(_) => Self::Conflict(error.to_string()),
            ProjectManagerError::Backend(source) => Self::Internal(source),
        }
    }
}

/// CRUD over projects.
///
/// `#[async_trait]` boxes the returned futures so the trait stays
/// dyn-compatible: implementations are held as `Box<dyn ProjectManager>` (or
/// `Arc<..>`) and chosen at runtime.
#[async_trait::async_trait]
pub trait ProjectManager: Send + Sync + 'static {
    /// Every project visible to this caller.
    async fn list_projects(&self) -> ProjectResult<Vec<Project>>;

    /// Creates a project called `name`, returning it as the backend recorded it.
    async fn create_project(&self, name: &str) -> ProjectResult<Project>;

    /// Renames `project_id`, returning the project as it now stands.
    async fn update_project(&self, project_id: &ProjectId, name: &str) -> ProjectResult<Project>;

    /// Deletes `project_id`, returning the id that was deleted.
    async fn delete_project(&self, project_id: &ProjectId) -> ProjectResult<ProjectId>;
}
