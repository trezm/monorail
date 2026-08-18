//! Railway (<https://railway.com>) as a container backend.
//!
//! Railway deploys a service straight from a GitHub repository, which lines up

use crate::services::{
    container::{ContainerId, ContainerManager, ContainerResult},
    project::{Project, ProjectId, ProjectManager, ProjectResult},
};

/// Talks to the Railway API on our behalf.
///
/// Empty for now. Credentials and an HTTP client will land here, which is why
/// it is constructed through [`Railway::new`] rather than as a bare literal —
/// callers will not have to change when it gains fields.
#[derive(Debug, Clone, Default)]
pub struct Railway;

impl Railway {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ContainerManager for Railway {
    async fn create_container(&self, _github_url: &str) -> ContainerResult<ContainerId> {
        todo!("deploy the repository as a Railway service")
    }

    async fn destroy_container(&self, _container_id: &ContainerId) -> ContainerResult<ContainerId> {
        todo!("delete the Railway service")
    }
}

#[async_trait::async_trait]
impl ProjectManager for Railway {
    async fn list_projects(&self) -> ProjectResult<Vec<Project>> {
        todo!("list the caller's Railway projects")
    }

    async fn create_project(&self, _name: &str) -> ProjectResult<Project> {
        todo!("create a Railway project")
    }

    async fn update_project(&self, _project_id: &ProjectId, _name: &str) -> ProjectResult<Project> {
        todo!("rename a Railway project")
    }

    async fn delete_project(&self, _project_id: &ProjectId) -> ProjectResult<ProjectId> {
        todo!("delete a Railway project")
    }
}
