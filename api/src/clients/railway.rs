//! Railway (<https://railway.com>) as a container backend.
//!
//! Railway deploys a service straight from a GitHub repository, which lines up
//! with what [`ContainerManager`] asks for. The client is a stub: it holds no
//! state and every method is unimplemented.

use crate::services::container::{ContainerId, ContainerManager, ContainerResult};

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
