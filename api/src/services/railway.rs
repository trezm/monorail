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
//! assembles. Environments and service instances are separate queries because
//! they are read on demand: an instance is keyed by service *and* environment,
//! and fetching every combination up front is exactly the oversized query this
//! surface has answered with a `503` before.

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

    /// The thing asked about does not exist on Railway — a service with no
    /// instance in the requested environment, an id that names nothing.
    #[error("{0}")]
    NotFound(String),

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
            RailwayError::NotFound(message) => Self::NotFound(message),
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

/// One environment of a [`Project`] — production, staging, whatever else the
/// project defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
}

/// How one [`Service`] is configured and deployed in one [`Environment`].
///
/// Most fields are optional on Railway's side: an instance that has never
/// overridden a setting reports `null` for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceInstance {
    pub id: String,
    pub start_command: Option<String>,
    pub build_command: Option<String>,
    pub root_directory: Option<String>,
    pub healthcheck_path: Option<String>,
    pub region: Option<String>,
    pub num_replicas: Option<i64>,
    pub restart_policy_type: Option<String>,
    pub restart_policy_max_retries: Option<i64>,
    pub latest_deployment: Option<Deployment>,
}

/// The most recent deployment of a [`ServiceInstance`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deployment {
    pub id: String,
    pub status: String,
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
/// what lets the route tests answer with `MockRailwayApi` and no network.
#[cfg_attr(test, mockall::automock)]
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

    /// The durable environments of one project. The ephemeral ones Railway
    /// creates per pull request are excluded.
    async fn environments(
        &self,
        access_token: &Secret,
        project_id: &str,
    ) -> RailwayResult<Vec<Environment>>;

    /// How a service is configured in one environment.
    /// [`RailwayError::NotFound`] when it has no instance there.
    async fn service_instance(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<ServiceInstance>;

    /// Spins a service down in one environment by removing its latest
    /// deployment. The service and its configuration survive; only the
    /// running thing goes away. [`RailwayError::NotFound`] when the service
    /// has no instance there, [`RailwayError::Rejected`] when nothing is
    /// running to remove.
    async fn spin_down(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<()>;
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

/// `isEphemeral: false` drops the environments Railway creates per pull
/// request; they churn too fast to be worth a place in a dropdown.
const ENVIRONMENTS_QUERY: &str = r"
query Environments($projectId: String!) {
  environments(projectId: $projectId, isEphemeral: false) {
    edges { node { id name createdAt } }
  }
}
";

const SERVICE_INSTANCE_QUERY: &str = r"
query ServiceInstance($serviceId: String!, $environmentId: String!) {
  serviceInstance(serviceId: $serviceId, environmentId: $environmentId) {
    id
    startCommand
    buildCommand
    rootDirectory
    healthcheckPath
    region
    numReplicas
    restartPolicyType
    restartPolicyMaxRetries
    latestDeployment { id status createdAt }
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

/// Spinning down is Railway's own "Remove" on a deployment: the instance and
/// its configuration stay, and `serviceInstanceRedeploy` brings it back later.
const DEPLOYMENT_REMOVE_MUTATION: &str = r"
mutation DeploymentRemove($id: String!) {
  deploymentRemove(id: $id)
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
            .into_data(query_rejected("projects"))?
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

    async fn environments(
        &self,
        access_token: &Secret,
        project_id: &str,
    ) -> RailwayResult<Vec<Environment>> {
        let body = serde_json::json!({
            "query": ENVIRONMENTS_QUERY,
            "variables": { "projectId": project_id },
        });

        let envelope: GraphQlResponse<EnvironmentsQuery> =
            self.post(access_token, &body, "environments").await?;

        Ok(envelope
            .into_data(query_rejected("environments"))?
            .into_environments())
    }

    async fn service_instance(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<ServiceInstance> {
        let body = serde_json::json!({
            "query": SERVICE_INSTANCE_QUERY,
            "variables": {
                "serviceId": service_id,
                "environmentId": environment_id,
            },
        });

        let envelope: GraphQlResponse<ServiceInstanceQuery> =
            self.post(access_token, &body, "service instance").await?;

        // Asking about a service with no instance in the environment is an
        // error entry with a `200`, not a null — and a `404`'s worth of error,
        // not an unhealthy provider.
        let data = envelope.into_data(|messages| {
            if is_missing_resource(&messages) {
                RailwayError::NotFound(messages)
            } else {
                query_rejected("service instance")(messages)
            }
        })?;

        Ok(data.service_instance.into_instance())
    }

    /// The mutation wants a deployment id, but callers think in services and
    /// environments — so the instance is read first to find what is running.
    async fn spin_down(
        &self,
        access_token: &Secret,
        service_id: &str,
        environment_id: &str,
    ) -> RailwayResult<()> {
        let instance = self
            .service_instance(access_token, service_id, environment_id)
            .await?;

        // The latest deployment is what gets removed — unless the service has
        // nothing running, which is the caller's situation to hear about, not
        // a provider fault.
        let deployment = match instance.latest_deployment {
            Some(deployment)
                if deployment.status != "REMOVED" && deployment.status != "REMOVING" =>
            {
                deployment
            }
            Some(_) => {
                return Err(RailwayError::Rejected(
                    "the service is already spun down in this environment".to_owned(),
                ));
            }
            None => {
                return Err(RailwayError::Rejected(
                    "the service has never been deployed in this environment".to_owned(),
                ));
            }
        };

        let body = serde_json::json!({
            "query": DEPLOYMENT_REMOVE_MUTATION,
            "variables": { "id": deployment.id },
        });

        let envelope: GraphQlResponse<DeploymentRemoveMutation> =
            self.post(access_token, &body, "deployment removal").await?;

        if envelope
            .into_data(RailwayError::Rejected)?
            .deployment_remove
        {
            Ok(())
        } else {
            Err(RailwayError::Provider(anyhow::anyhow!(
                "Railway answered the deployment removal with a refusal it did not explain"
            )))
        }
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

/// The [`GraphQlResponse::into_data`] rejection for a read: a query Railway
/// declines is the provider's problem, not the caller's.
fn query_rejected(operation: &'static str) -> impl FnOnce(String) -> RailwayError {
    move |messages| {
        RailwayError::Provider(anyhow::anyhow!(
            "Railway rejected the {operation} query: {messages}"
        ))
    }
}

fn is_authorization_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    message.contains("not authorized")
        || message.contains("unauthorized")
        || message.contains("unauthenticated")
}

fn is_missing_resource(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    message.contains("not found") || message.contains("does not exist")
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentRemoveMutation {
    deployment_remove: bool,
}

#[derive(Debug, Deserialize)]
struct EnvironmentsQuery {
    environments: Connection<EnvironmentNode>,
}

impl EnvironmentsQuery {
    /// Sorted by name, like the projects, and for the same reason.
    fn into_environments(self) -> Vec<Environment> {
        let mut environments: Vec<Environment> = self
            .environments
            .edges
            .into_iter()
            .map(|edge| Environment {
                id: edge.node.id,
                name: edge.node.name,
                created_at: edge.node.created_at,
            })
            .collect();

        environments.sort_by(|left, right| left.name.cmp(&right.name));

        environments
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentNode {
    id: String,
    name: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceInstanceQuery {
    service_instance: ServiceInstanceNode,
}

/// Every field beyond `id` defaults rather than fails: the dashboard losing
/// one detail Railway reshaped is better than losing the whole instance.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceInstanceNode {
    id: String,
    #[serde(default)]
    start_command: Option<String>,
    #[serde(default)]
    build_command: Option<String>,
    #[serde(default)]
    root_directory: Option<String>,
    #[serde(default)]
    healthcheck_path: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    num_replicas: Option<i64>,
    #[serde(default)]
    restart_policy_type: Option<String>,
    #[serde(default)]
    restart_policy_max_retries: Option<i64>,
    #[serde(default)]
    latest_deployment: Option<DeploymentNode>,
}

impl ServiceInstanceNode {
    fn into_instance(self) -> ServiceInstance {
        ServiceInstance {
            id: self.id,
            start_command: self.start_command,
            build_command: self.build_command,
            root_directory: self.root_directory,
            healthcheck_path: self.healthcheck_path,
            region: self.region,
            num_replicas: self.num_replicas,
            restart_policy_type: self.restart_policy_type,
            restart_policy_max_retries: self.restart_policy_max_retries,
            latest_deployment: self.latest_deployment.map(|node| Deployment {
                id: node.id,
                status: node.status,
                created_at: node.created_at,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentNode {
    id: String,
    status: String,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> RailwayResult<Vec<Project>> {
        serde_json::from_str::<GraphQlResponse<ExternalWorkspacesQuery>>(body)
            .expect("body should parse")
            .into_data(query_rejected("projects"))
            .map(ExternalWorkspacesQuery::into_projects)
    }

    fn parse_environments(body: &str) -> RailwayResult<Vec<Environment>> {
        serde_json::from_str::<GraphQlResponse<EnvironmentsQuery>>(body)
            .expect("body should parse")
            .into_data(query_rejected("environments"))
            .map(EnvironmentsQuery::into_environments)
    }

    fn parse_instance(body: &str) -> RailwayResult<ServiceInstance> {
        serde_json::from_str::<GraphQlResponse<ServiceInstanceQuery>>(body)
            .expect("body should parse")
            .into_data(|messages| {
                if is_missing_resource(&messages) {
                    RailwayError::NotFound(messages)
                } else {
                    query_rejected("service instance")(messages)
                }
            })
            .map(|query| query.service_instance.into_instance())
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

    #[test]
    fn environments_come_back_sorted_by_name() {
        let environments = parse_environments(
            r#"{"data":{"environments":{"edges":[
                {"node":{"id":"e2","name":"staging","createdAt":null}},
                {"node":{"id":"e1","name":"production","createdAt":null}}]}}}"#,
        )
        .expect("should parse");

        let names: Vec<_> = environments
            .iter()
            .map(|environment| environment.name.as_str())
            .collect();
        assert_eq!(names, ["production", "staging"]);
    }

    #[test]
    fn an_instance_parses_with_every_optional_field_absent() {
        let instance =
            parse_instance(r#"{"data":{"serviceInstance":{"id":"i1"}}}"#).expect("should parse");

        assert_eq!(instance.id, "i1");
        assert_eq!(instance.start_command, None);
        assert_eq!(instance.latest_deployment, None);
    }

    #[test]
    fn an_instance_carries_its_latest_deployment() {
        let instance = parse_instance(
            r#"{"data":{"serviceInstance":{
                "id":"i1","startCommand":"cargo run","region":"us-west2",
                "numReplicas":2,"restartPolicyType":"ON_FAILURE",
                "restartPolicyMaxRetries":10,
                "latestDeployment":{"id":"d1","status":"SUCCESS","createdAt":null}}}}"#,
        )
        .expect("should parse");

        assert_eq!(instance.region.as_deref(), Some("us-west2"));
        assert_eq!(instance.num_replicas, Some(2));

        let deployment = instance.latest_deployment.expect("should be present");
        assert_eq!(deployment.status, "SUCCESS");
    }

    #[test]
    fn a_declined_removal_carries_railways_message() {
        let error = serde_json::from_str::<GraphQlResponse<DeploymentRemoveMutation>>(
            r#"{"data":null,"errors":[{"message":"Deployment not found"}]}"#,
        )
        .expect("body should parse")
        .into_data(RailwayError::Rejected)
        .expect_err("should fail");

        assert!(
            matches!(error, RailwayError::Rejected(message) if message == "Deployment not found")
        );
    }

    /// A service with no instance in the asked-about environment is a `404`'s
    /// worth of error, not an unhealthy provider.
    #[test]
    fn a_missing_instance_is_not_found() {
        let error =
            parse_instance(r#"{"data":null,"errors":[{"message":"Service instance not found"}]}"#)
                .expect_err("should fail");

        assert!(matches!(error, RailwayError::NotFound(_)));
    }
}
