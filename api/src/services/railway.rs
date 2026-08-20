//! Railway's resources, over its public GraphQL API.
//!
//! Distinct from [`crate::services::auth`], which only establishes who someone
//! is: this is what the `project:member` and `workspace:member` scopes on that
//! login are for. Both talk to the same host, and the endpoint below is derived
//! from the same configured issuer, so pointing a test at a local server points
//! both.
//!
//! The API is GraphQL, and one query returns projects with their services
//! nested inside — which is why [`RailwayApi`] has no separate `services()`.
//! Asking twice would be two round trips for a shape the server already
//! assembles.

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{config::OAuthConfig, error::ApiError, secret::Secret};

pub type RailwayResult<T> = Result<T, RailwayError>;

/// What can go wrong while reading Railway.
///
/// Independent of [`ApiError`] for the same reason every other service's error
/// is; the `From` impl below is the one place that decides how each case
/// reaches a client.
#[derive(Debug, thiserror::Error)]
pub enum RailwayError {
    /// Railway would not accept the access token. It has been revoked, or the
    /// consent it was granted under no longer covers the query.
    #[error("this Railway login is no longer valid; sign in again")]
    TokenRejected,

    /// Railway understood a mutation and declined it — a project that does not
    /// exist, a source it cannot use. The message is Railway's own, and it is
    /// the caller's to act on, not an outage.
    #[error("{0}")]
    Rejected(String),

    /// Railway was reached but did not answer usefully.
    #[error(transparent)]
    Provider(anyhow::Error),
}

impl From<RailwayError> for ApiError {
    fn from(error: RailwayError) -> Self {
        match error {
            // A `401` and not a `403`: the browser's own session is fine, but
            // the credential behind it is spent, and starting a new login is
            // what fixes it.
            RailwayError::TokenRejected => Self::Unauthorized,
            RailwayError::Rejected(message) => Self::UnprocessableEntity(message),
            RailwayError::Provider(source) => Self::Unavailable(source),
        }
    }
}

/// A Railway project, with the services deployed in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub services: Vec<Service>,
}

/// One service inside a [`Project`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Service {
    pub id: String,
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
}

/// Where a new service's code comes from — the only two sources creation
/// accepts today. Externally tagged, so on the wire it reads
/// `{"docker_image": "nginx:latest"}` or `{"github_repo": "owner/repo"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceSource {
    DockerImage(String),
    GithubRepo(String),
}

impl ServiceSource {
    /// The image or repo as the caller gave it, without surrounding whitespace.
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::DockerImage(value) | Self::GithubRepo(value) => value.trim(),
        }
    }

    /// The wire name of the variant, for messages about it.
    #[must_use]
    pub fn field(&self) -> &'static str {
        match self {
            Self::DockerImage(_) => "docker_image",
            Self::GithubRepo(_) => "github_repo",
        }
    }
}

/// Reads Railway on behalf of a logged-in user.
///
/// A trait for the same reason [`AuthProvider`](super::auth::AuthProvider) is
/// one — handlers depend on the behaviour, not on GraphQL — and because it is
/// what lets the route tests answer without a network.
#[async_trait::async_trait]
pub trait RailwayApi: Send + Sync + 'static {
    /// Every project the login granted access to, each carrying its services.
    async fn projects(&self, access_token: &Secret) -> RailwayResult<Vec<Project>>;

    /// Creates a service in `project_id` from `source`, returning it as Railway
    /// records it. Everything not in the source — the name included — is left
    /// to Railway's defaults.
    async fn create_service(
        &self,
        access_token: &Secret,
        project_id: &str,
        source: &ServiceSource,
    ) -> RailwayResult<Service>;
}

/// The query. `externalWorkspaces` is the surface Railway documents for OAuth
/// tokens — it returns exactly the workspaces and projects picked on the
/// consent screen, as plain lists. Only `services` is a Relay connection,
/// flattened on the way out.
const PROJECTS_QUERY: &str = r"
query Projects {
  externalWorkspaces {
    id
    name
    projects {
      id
      name
      description
      createdAt
      services { edges { node { id name createdAt } } }
    }
  }
}
";

/// The mutation. `source` takes exactly one of `image` or `repo` — the same
/// fork [`ServiceSource`] exposes; every other input is left unset.
const SERVICE_CREATE_MUTATION: &str = r"
mutation ServiceCreate($input: ServiceCreateInput!) {
  serviceCreate(input: $input) {
    id
    name
    createdAt
  }
}
";

fn service_create_variables(project_id: &str, source: &ServiceSource) -> serde_json::Value {
    let source = match source {
        ServiceSource::DockerImage(_) => serde_json::json!({ "image": source.value() }),
        ServiceSource::GithubRepo(_) => serde_json::json!({ "repo": source.value() }),
    };

    serde_json::json!({ "input": { "projectId": project_id, "source": source } })
}

/// [`RailwayApi`] against Railway's public GraphQL API.
pub struct RailwayGraphQl {
    endpoint: Url,
    http: reqwest::Client,
}

impl RailwayGraphQl {
    /// Shares the OAuth configuration rather than taking its own: the endpoint
    /// is the issuer's, and a token minted against one host is worthless
    /// against another.
    pub fn new(config: &OAuthConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("could not build the Railway HTTP client")?;

        Ok(Self {
            endpoint: config
                .issuer
                .join(crate::constants::RAILWAY_GRAPHQL_PATH)
                .context("issuer does not join to a GraphQL URL")?,
            http,
        })
    }
}

impl RailwayGraphQl {
    /// Posts one GraphQL document and parses the envelope. Transport failures,
    /// non-success statuses and unparseable bodies are all the provider's
    /// fault; what a `200` envelope's `errors` mean is the caller's call.
    /// `operation` names the request in error messages.
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        access_token: &Secret,
        body: &serde_json::Value,
        operation: &str,
    ) -> RailwayResult<GraphQlResponse<T>> {
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(access_token.expose())
            .json(body)
            .send()
            .await
            .map_err(|error| {
                RailwayError::Provider(
                    anyhow::Error::new(error).context(format!("the {operation} request failed")),
                )
            })?;

        let status = response.status();
        let body = response.bytes().await.map_err(|error| {
            RailwayError::Provider(
                anyhow::Error::new(error)
                    .context(format!("could not read the {operation} response")),
            )
        })?;

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(RailwayError::TokenRejected);
        }

        if !status.is_success() {
            return Err(RailwayError::Provider(anyhow::anyhow!(
                "Railway answered {status} to the {operation} request"
            )));
        }

        serde_json::from_slice(&body).map_err(|error| {
            RailwayError::Provider(anyhow::Error::new(error).context(format!(
                "the {operation} response was not the expected shape"
            )))
        })
    }
}

#[async_trait::async_trait]
impl RailwayApi for RailwayGraphQl {
    async fn projects(&self, access_token: &Secret) -> RailwayResult<Vec<Project>> {
        let envelope: GraphQlResponse<ExternalWorkspacesQuery> = self
            .post(
                access_token,
                &serde_json::json!({ "query": PROJECTS_QUERY }),
                "projects",
            )
            .await?;

        Ok(envelope
            .into_data(|messages| {
                RailwayError::Provider(anyhow::anyhow!(
                    "Railway rejected the projects query: {messages}"
                ))
            })?
            .into_projects())
    }

    async fn create_service(
        &self,
        access_token: &Secret,
        project_id: &str,
        source: &ServiceSource,
    ) -> RailwayResult<Service> {
        let body = serde_json::json!({
            "query": SERVICE_CREATE_MUTATION,
            "variables": service_create_variables(project_id, source),
        });

        let envelope: GraphQlResponse<ServiceCreateMutation> =
            self.post(access_token, &body, "service creation").await?;

        let node = envelope.into_data(RailwayError::Rejected)?.service_create;

        Ok(Service {
            id: node.id,
            name: node.name,
            created_at: node.created_at,
        })
    }
}

/// A GraphQL response. `data` and `errors` can both be present — a partial
/// result with a failed field — so an empty `errors` is what success means,
/// not the presence of `data`.
#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    #[serde(default = "Option::default")]
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

impl<T> GraphQlResponse<T> {
    /// `reject` decides what a non-authorization error becomes: a failed query
    /// is the provider's problem, a declined mutation is the caller's.
    fn into_data(self, reject: impl FnOnce(String) -> RailwayError) -> RailwayResult<T> {
        if !self.errors.is_empty() {
            let messages: Vec<_> = self
                .errors
                .iter()
                .map(|error| error.message.as_str())
                .collect();

            // GraphQL reports an unusable credential in the body with a `200`,
            // so the status code alone does not catch it.
            if messages
                .iter()
                .any(|message| is_authorization_failure(message))
            {
                return Err(RailwayError::TokenRejected);
            }

            return Err(reject(messages.join("; ")));
        }

        self.data
            .ok_or_else(|| RailwayError::Provider(anyhow::anyhow!("Railway returned no data")))
    }
}

fn is_authorization_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    message.contains("not authorized")
        || message.contains("unauthorized")
        || message.contains("unauthenticated")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalWorkspacesQuery {
    external_workspaces: Vec<ExternalWorkspace>,
}

#[derive(Debug, Deserialize)]
struct ExternalWorkspace {
    projects: Vec<ProjectNode>,
}

impl ExternalWorkspacesQuery {
    /// Both levels are sorted by name, so the dashboard does not reshuffle
    /// between requests over an order the API never promised.
    fn into_projects(self) -> Vec<Project> {
        let mut projects: Vec<Project> = self
            .external_workspaces
            .into_iter()
            .flat_map(|workspace| workspace.projects)
            .map(|node| {
                let mut services: Vec<Service> = node
                    .services
                    .edges
                    .into_iter()
                    .map(|edge| Service {
                        id: edge.node.id,
                        name: edge.node.name,
                        created_at: edge.node.created_at,
                    })
                    .collect();

                services.sort_by(|left, right| left.name.cmp(&right.name));

                Project {
                    id: node.id,
                    name: node.name,
                    description: node.description,
                    created_at: node.created_at,
                    services,
                }
            })
            .collect();

        projects.sort_by(|left, right| left.name.cmp(&right.name));

        projects
    }
}

#[derive(Debug, Deserialize)]
struct Connection<T> {
    #[serde(default = "Vec::new")]
    edges: Vec<Edge<T>>,
}

#[derive(Debug, Deserialize)]
struct Edge<T> {
    node: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectNode {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    services: Connection<ServiceNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceNode {
    id: String,
    name: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceCreateMutation {
    service_create: ServiceNode,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> RailwayResult<Vec<Project>> {
        serde_json::from_str::<GraphQlResponse<ExternalWorkspacesQuery>>(body)
            .expect("body should parse")
            .into_data(|messages| {
                RailwayError::Provider(anyhow::anyhow!(
                    "Railway rejected the projects query: {messages}"
                ))
            })
            .map(ExternalWorkspacesQuery::into_projects)
    }

    #[test]
    fn projects_and_services_come_back_sorted_by_name() {
        let projects = parse(
            r#"{"data":{"externalWorkspaces":[
                {"id":"w1","name":"one","projects":[
                    {"id":"p2","name":"zeta","description":null,"createdAt":null,
                      "services":{"edges":[
                        {"node":{"id":"s2","name":"worker","createdAt":null}},
                        {"node":{"id":"s1","name":"api","createdAt":null}}]}}]},
                {"id":"w2","name":"two","projects":[
                    {"id":"p1","name":"alpha","description":"first","createdAt":null,
                      "services":{"edges":[]}}]}]}}"#,
        )
        .expect("should parse");

        let names: Vec<_> = projects
            .iter()
            .map(|project| project.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "zeta"]);

        let services: Vec<_> = projects[1]
            .services
            .iter()
            .map(|service| service.name.as_str())
            .collect();
        assert_eq!(services, ["api", "worker"]);
    }

    /// Nothing granted on the consent screen is an empty dashboard, not an
    /// error — whether no workspaces came back or one came back empty.
    #[test]
    fn no_granted_projects_is_an_empty_list() {
        let projects = parse(r#"{"data":{"externalWorkspaces":[]}}"#).expect("should parse");
        assert!(projects.is_empty());

        let projects =
            parse(r#"{"data":{"externalWorkspaces":[{"projects":[]}]}}"#).expect("should parse");
        assert!(projects.is_empty());
    }

    #[test]
    fn an_authorization_error_asks_for_a_new_login() {
        let error = parse(r#"{"data":null,"errors":[{"message":"Not Authorized"}]}"#)
            .expect_err("should fail");

        assert!(matches!(error, RailwayError::TokenRejected));
    }

    /// Anything else is Railway's problem, not the caller's credential.
    #[test]
    fn any_other_error_is_an_unhealthy_provider() {
        let error =
            parse(r#"{"errors":[{"message":"internal server error"}]}"#).expect_err("should fail");

        assert!(matches!(error, RailwayError::Provider(_)));
    }

    #[test]
    fn a_body_with_neither_data_nor_errors_is_a_failure() {
        let error = parse("{}").expect_err("should fail");

        assert!(matches!(error, RailwayError::Provider(_)));
    }

    #[test]
    fn each_source_fills_its_own_input_field() {
        let image = service_create_variables(
            "p1",
            &ServiceSource::DockerImage(" nginx:latest ".to_owned()),
        );
        assert_eq!(
            image,
            serde_json::json!({ "input": { "projectId": "p1", "source": { "image": "nginx:latest" } } })
        );

        let repo =
            service_create_variables("p1", &ServiceSource::GithubRepo("owner/repo".to_owned()));
        assert_eq!(
            repo,
            serde_json::json!({ "input": { "projectId": "p1", "source": { "repo": "owner/repo" } } })
        );
    }

    #[test]
    fn a_declined_mutation_carries_railways_message() {
        let error = serde_json::from_str::<GraphQlResponse<ServiceCreateMutation>>(
            r#"{"data":null,"errors":[{"message":"Project not found"}]}"#,
        )
        .expect("body should parse")
        .into_data(RailwayError::Rejected)
        .expect_err("should fail");

        assert!(matches!(error, RailwayError::Rejected(message) if message == "Project not found"));
    }
}
