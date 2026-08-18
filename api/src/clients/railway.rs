//! Railway (<https://railway.com>) as a service backend.
//!
//! Railway deploys a service straight from a GitHub repository, which lines up
//! with what [`ServiceManager`] asks for, and groups those deployments into
//! projects, which is what [`ProjectManager`] asks for.
//!
//! Railway's public API is GraphQL, so this holds the HTTP client that will
//! carry those queries, and the endpoint it sends them to. The trait methods
//! are still unimplemented.

use std::time::Duration;

use crate::services::{
    project::{Project, ProjectId, ProjectManager, ProjectResult},
    service::{ServiceId, ServiceManager, ServiceResult},
};

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
    /// `endpoint` comes from [`Config::railway_endpoint`](crate::config::Config),
    /// which defaults to Railway's public API and is overridable with
    /// `API_RAILWAY_ENDPOINT` for a self-hosted install or a test stub.
    ///
    /// # Errors
    ///
    /// Fails if the TLS backend cannot be initialised, which in practice means
    /// a broken build rather than anything recoverable at runtime.
    pub fn new(endpoint: impl Into<String>) -> reqwest::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            endpoint: endpoint.into(),
        })
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
