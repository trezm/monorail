//! Railway (<https://railway.com>) as a service backend.
//!
//! Railway deploys a service straight from a GitHub repository, which lines up
//! with what [`ServiceManager`] asks for, and groups those deployments into
//! projects, which is what [`ProjectManager`] asks for.
//!
//! Railway's public API is GraphQL, so this holds the HTTP client that will
//! carry those queries, plus the endpoint and per-request timeout it is
//! configured with. Credentials are not held here: every trait method takes
//! the caller's access token, which will become the `Authorization` header
//! on the GraphQL request. The trait methods are still unimplemented.

use std::time::Duration;

use crate::services::{
    auth::AccessToken,
    project::{Project, ProjectId, ProjectManager, ProjectResult},
    service::{ServiceId, ServiceManager, ServiceResult},
};

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
    /// Both arguments come from [`Config`](crate::config::Config)
    /// (`railway_endpoint` and `railway_timeout`), so they are set by
    /// `API_RAILWAY_ENDPOINT` and `API_RAILWAY_TIMEOUT_SECS`.
    ///
    /// # Errors
    ///
    /// Fails if the TLS backend cannot be initialised, which in practice means
    /// a broken build rather than anything recoverable at runtime.
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> reqwest::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(timeout).build()?,
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
    async fn create_service(
        &self,
        _access_token: &AccessToken,
        _github_url: &str,
    ) -> ServiceResult<ServiceId> {
        todo!("deploy the repository as a Railway service")
    }

    async fn destroy_service(
        &self,
        _access_token: &AccessToken,
        _service_id: &ServiceId,
    ) -> ServiceResult<ServiceId> {
        todo!("delete the Railway service")
    }
}

#[async_trait::async_trait]
impl ProjectManager for Railway {
    async fn list_projects(&self, _access_token: &AccessToken) -> ProjectResult<Vec<Project>> {
        todo!("list the caller's Railway projects")
    }

    async fn create_project(
        &self,
        _access_token: &AccessToken,
        _name: &str,
    ) -> ProjectResult<Project> {
        todo!("create a Railway project")
    }

    async fn update_project(
        &self,
        _access_token: &AccessToken,
        _project_id: &ProjectId,
        _name: &str,
    ) -> ProjectResult<Project> {
        todo!("rename a Railway project")
    }

    async fn delete_project(
        &self,
        _access_token: &AccessToken,
        _project_id: &ProjectId,
    ) -> ProjectResult<ProjectId> {
        todo!("delete a Railway project")
    }
}
