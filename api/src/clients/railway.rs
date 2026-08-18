//! Railway (<https://railway.com>) as a service backend.
//!
//! Railway deploys a service straight from a GitHub repository, which lines up
//! with what [`ServiceManager`] asks for, and groups those deployments into
//! projects, which is what [`ProjectManager`] asks for.
//!
//! Railway's public API is GraphQL, so this holds the HTTP client that will
//! carry those queries. The trait methods are still unimplemented.

use std::time::Duration;

use crate::services::{
    project::{Project, ProjectId, ProjectManager, ProjectResult},
    service::{ServiceId, ServiceManager, ServiceResult},
};

/// Railway's GraphQL API endpoint.
const ENDPOINT: &str = "https://backboard.railway.com/graphql/v2";

/// How long a single Railway request may take before we give up on it.
///
/// Well under the API's own 30s request timeout, so a stalled upstream call
/// cannot hold a client connection open past it.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Talks to the Railway GraphQL API on our behalf.
///
/// `reqwest::Client` owns the connection pool, so this is cheap to clone and
/// expensive to rebuild — construct it once and share it.
#[derive(Debug, Clone)]
pub struct Railway {
    http: reqwest::Client,
    endpoint: String,
}

impl Railway {
    /// # Errors
    ///
    /// Fails if the TLS backend cannot be initialised, which in practice means
    /// a broken build rather than anything recoverable at runtime.
    pub fn new() -> reqwest::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            endpoint: ENDPOINT.to_owned(),
        })
    }

    /// Points the client at a different endpoint, for tests and self-hosted
    /// Railway installs.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

#[async_trait::async_trait]
impl ServiceManager for Railway {
    async fn create_service(&self, _github_url: &str) -> ServiceResult<ServiceId> {
        todo!("deploy the repository as a Railway service")
    }

    async fn destroy_service(&self, _service_id: &ServiceId) -> ServiceResult<ServiceId> {
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
