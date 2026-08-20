//! The signed-in user's Railway projects, and their environments.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    error::{ApiError, ApiResult},
    extract::{CurrentSession, Json, Path},
    services::railway::{Environment, Project, Service, ServiceSource},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list))
        .route("/projects/{project_id}/services", post(create_service))
        .route("/projects/{project_id}/environments", get(environments))
}

/// An object rather than a bare array, so a later addition — a cursor, a
/// workspace the projects belong to — is not a breaking change.
#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub projects: Vec<Project>,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentList {
    pub environments: Vec<Environment>,
}

/// Renewing the access token is [`Credentials`](crate::services::session::Credentials)'
/// job, not this one's: a session outlives by weeks the token it was opened
/// with, and every endpoint that acts on Railway needs the same answer.
async fn list(
    State(state): State<AppState>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<ProjectList>> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let projects = state.railway().projects(&access_token).await?;

    Ok(Json(ProjectList { projects }))
}

/// The body of `POST /projects/{project_id}/services`. An object holding the
/// source rather than the source alone, so a later knob — a name, a branch —
/// is an added field, not a reshaped body.
#[derive(Debug, Deserialize)]
pub struct NewService {
    pub source: ServiceSource,
}

/// Creation is a pass-through: the source is the only configurable thing, and
/// Railway owns every other setting — the name included.
async fn create_service(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    CurrentSession { token, session }: CurrentSession,
    Json(body): Json<NewService>,
) -> ApiResult<(StatusCode, Json<Service>)> {
    if body.source.value().is_empty() {
        return Err(ApiError::UnprocessableEntity(format!(
            "{} must not be empty",
            body.source.field()
        )));
    }

    let access_token = state.credentials().access_token(&token, session).await?;
    let service = state
        .railway()
        .create_service(&access_token, &project_id, &body.source)
        .await?;

    Ok((StatusCode::CREATED, Json(service)))
}

/// Separate from the projects list because environments are read on demand —
/// when a project is expanded — not for every project on every page load.
async fn environments(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<EnvironmentList>> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let environments = state
        .railway()
        .environments(&access_token, &project_id)
        .await?;

    Ok(Json(EnvironmentList { environments }))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::{
        secret::Secret,
        services::{
            auth::{AuthError, MockAuthProvider, TokenSet},
            railway::{MockRailwayApi, RailwayError, Service, ServiceSource},
            session::MockSessionStore,
        },
        testing,
    };

    fn created_service() -> Service {
        Service {
            id: "service-new".to_owned(),
            name: "shiny-new-service".to_owned(),
            created_at: None,
        }
    }

    #[tokio::test]
    async fn the_projects_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(&app, testing::get("/projects")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn the_environments_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) =
            testing::send(&app, testing::get("/projects/project-1/environments")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn creating_a_service_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(
            &app,
            testing::post_json(
                "/projects/project-1/services",
                None,
                &json!({ "source": { "docker_image": "nginx:latest" } }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn a_service_is_created_from_a_github_repo() {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(&mut sessions, testing::fresh_tokens());

        let mut railway = MockRailwayApi::new();
        railway
            .expect_create_service()
            .withf(|_, project_id, source| {
                project_id == "project-1"
                    && *source == ServiceSource::GithubRepo("railwayapp/starters".to_owned())
            })
            .times(1)
            .returning(|_, _, _| Ok(created_service()));

        let app =
            super::router().with_state(testing::state(MockAuthProvider::new(), sessions, railway));

        let (status, body) = testing::send(
            &app,
            testing::post_json(
                "/projects/project-1/services",
                Some(&cookie),
                &json!({ "source": { "github_repo": "railwayapp/starters" } }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["id"], "service-new");
    }

    #[tokio::test]
    async fn a_blank_source_never_reaches_railway() {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(&mut sessions, testing::fresh_tokens());

        let mut railway = MockRailwayApi::new();
        railway.expect_create_service().never();

        let app =
            super::router().with_state(testing::state(MockAuthProvider::new(), sessions, railway));

        let (status, body) = testing::send(
            &app,
            testing::post_json(
                "/projects/project-1/services",
                Some(&cookie),
                &json!({ "source": { "docker_image": "   " } }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "unprocessable_entity");
        assert_eq!(body["error"]["message"], "docker_image must not be empty");
    }

    /// Only the two supported sources deserialize; anything else is caught by
    /// the extractor and answers on the standard envelope.
    #[tokio::test]
    async fn an_unsupported_source_kind_is_rejected() {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(&mut sessions, testing::fresh_tokens());

        let mut railway = MockRailwayApi::new();
        railway.expect_create_service().never();

        let app =
            super::router().with_state(testing::state(MockAuthProvider::new(), sessions, railway));

        let (status, body) = testing::send(
            &app,
            testing::post_json(
                "/projects/project-1/services",
                Some(&cookie),
                &json!({ "source": { "helm_chart": "bitnami/nginx" } }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "unprocessable_entity");
    }

    /// Railway declining the mutation is the caller's problem to fix, and its
    /// message survives the trip — not a `503` pretending the provider is
    /// down.
    #[tokio::test]
    async fn railways_rejection_reaches_the_caller() {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(&mut sessions, testing::fresh_tokens());

        let mut railway = MockRailwayApi::new();
        railway
            .expect_create_service()
            .returning(|_, _, _| Err(RailwayError::Rejected("Project not found".to_owned())));

        let app =
            super::router().with_state(testing::state(MockAuthProvider::new(), sessions, railway));

        let (status, body) = testing::send(
            &app,
            testing::post_json(
                "/projects/project-forbidden/services",
                Some(&cookie),
                &json!({ "source": { "docker_image": "nginx:latest" } }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["message"], "Project not found");
    }

    /// A session outlives the access token it was opened with, so a stale one
    /// is renewed in place — and written back without losing a refresh token
    /// the provider did not rotate — rather than logging the user out.
    #[tokio::test]
    async fn an_expired_access_token_is_renewed_before_railway_is_read() {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(
            &mut sessions,
            testing::expired_tokens(Some(Secret::new("refresh-ok"))),
        );
        sessions
            .expect_renew()
            .withf(|_, tokens| {
                tokens.access_token.expose() == "access-renewed"
                    && tokens.refresh_token.as_ref().map(Secret::expose) == Some("refresh-ok")
            })
            .times(1)
            .returning(|_, _| Ok(()));

        let mut auth = MockAuthProvider::new();
        auth.expect_refresh()
            .withf(|refresh_token| refresh_token.expose() == "refresh-ok")
            .times(1)
            .returning(|_| {
                Ok(TokenSet {
                    access_token: Secret::new("access-renewed"),
                    refresh_token: None,
                    id_token: None,
                    scope: "openid email".to_owned(),
                    expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
                })
            });

        let mut railway = MockRailwayApi::new();
        railway
            .expect_projects()
            .withf(|access_token| access_token.expose() == "access-renewed")
            .times(1)
            .returning(|_| Ok(Vec::new()));

        let app = super::router().with_state(testing::state(auth, sessions, railway));

        let (status, _) = testing::send(&app, testing::get_with_cookie("/projects", &cookie)).await;

        assert_eq!(status, StatusCode::OK);
    }

    /// A login that came back without a refresh token, or with one the
    /// provider no longer honours, has nothing left to renew: both send the
    /// browser back through a login rather than reporting a bad request, and
    /// the spent token never reaches Railway.
    #[tokio::test]
    async fn an_unrenewable_access_token_asks_for_a_new_login() {
        for tokens in [
            testing::expired_tokens(None),
            testing::expired_tokens(Some(Secret::new("refresh-revoked"))),
        ] {
            let mut sessions = MockSessionStore::new();
            let cookie = testing::logged_in(&mut sessions, tokens);

            let mut auth = MockAuthProvider::new();
            auth.expect_refresh()
                .returning(|_| Err(AuthError::InvalidGrant));

            let mut railway = MockRailwayApi::new();
            railway.expect_projects().never();

            let app = super::router().with_state(testing::state(auth, sessions, railway));

            let (status, body) =
                testing::send(&app, testing::get_with_cookie("/projects", &cookie)).await;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["error"]["code"], "unauthorized");
        }
    }
}
