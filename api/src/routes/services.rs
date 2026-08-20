//! A Railway service's per-environment instance, and actions on it.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;

use crate::{
    error::ApiResult,
    extract::{CurrentSession, Json, Path, Query},
    services::railway::{Deployment, ServiceInstance},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/services/{service_id}/instance", get(instance))
        .route("/services/{service_id}/spin-down", post(spin_down))
        .route("/services/{service_id}/spin-up", post(spin_up))
}

/// The environment rides in the query string rather than the path because it
/// is the axis the UI's dropdown varies, not a resource this route owns.
#[derive(Debug, Deserialize)]
struct InstanceQuery {
    environment: String,
}

/// `404` when the service has no instance in that environment — a service does
/// not have to be deployed everywhere its project has environments.
async fn instance(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<Json<ServiceInstance>> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let instance = state
        .railway()
        .service_instance(&access_token, &service_id, &query.environment)
        .await?;

    Ok(Json(instance))
}

/// Removes the service's latest deployment in that environment — a spin-down,
/// not a delete: the service and its configuration stay. `204` because removal
/// leaves nothing to describe; the UI refetches the instance for the
/// deployment's new state.
async fn spin_down(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<StatusCode> {
    let access_token = state.credentials().access_token(&token, session).await?;
    state
        .railway()
        .spin_down(&access_token, &service_id, &query.environment)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Redeploys what a spin-down removed — the inverse of `spin_down`, but `201`
/// where that is `204`: this creates a deployment, and the fresh one comes
/// back as Railway records it.
async fn spin_up(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(query): Query<InstanceQuery>,
    CurrentSession { token, session }: CurrentSession,
) -> ApiResult<(StatusCode, Json<Deployment>)> {
    let access_token = state.credentials().access_token(&token, session).await?;
    let deployment = state
        .railway()
        .spin_up(&access_token, &service_id, &query.environment)
        .await?;

    Ok((StatusCode::CREATED, Json(deployment)))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use crate::{
        services::{
            auth::MockAuthProvider,
            railway::{MockRailwayApi, RailwayError},
            session::MockSessionStore,
        },
        testing,
    };

    fn app_with_railway(railway: MockRailwayApi) -> (axum::Router, String) {
        let mut sessions = MockSessionStore::new();
        let cookie = testing::logged_in(&mut sessions, testing::fresh_tokens());
        let app =
            super::router().with_state(testing::state(MockAuthProvider::new(), sessions, railway));

        (app, cookie)
    }

    #[tokio::test]
    async fn the_instance_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(
            &app,
            testing::get("/services/service-1/instance?environment=env-1"),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn an_instance_request_without_an_environment_is_rejected() {
        let mut railway = MockRailwayApi::new();
        railway.expect_service_instance().never();
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::get_with_cookie("/services/service-1/instance", &cookie),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn a_service_missing_from_an_environment_is_not_found() {
        let mut railway = MockRailwayApi::new();
        railway
            .expect_service_instance()
            .withf(|_, service_id, environment_id| {
                service_id == "service-1" && environment_id == "env-empty"
            })
            .returning(|_, service_id, environment_id| {
                Err(RailwayError::NotFound(format!(
                    "service `{service_id}` has no instance in `{environment_id}`"
                )))
            });
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::get_with_cookie(
                "/services/service-1/instance?environment=env-empty",
                &cookie,
            ),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn the_spin_down_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(
            &app,
            testing::post_empty("/services/service-1/spin-down?environment=env-1", None),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn a_spin_down_without_an_environment_is_rejected() {
        let mut railway = MockRailwayApi::new();
        railway.expect_spin_down().never();
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty("/services/service-1/spin-down", Some(&cookie)),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn spinning_down_a_service_missing_from_an_environment_is_not_found() {
        let mut railway = MockRailwayApi::new();
        railway
            .expect_spin_down()
            .returning(|_, service_id, environment_id| {
                Err(RailwayError::NotFound(format!(
                    "service `{service_id}` has no instance in `{environment_id}`"
                )))
            });
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty(
                "/services/service-1/spin-down?environment=env-empty",
                Some(&cookie),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    /// Nothing running is the caller's situation, answered with Railway's own
    /// message — not a `503` pretending the provider is down.
    #[tokio::test]
    async fn spinning_down_a_parked_service_is_rejected() {
        let mut railway = MockRailwayApi::new();
        railway.expect_spin_down().returning(|_, _, _| {
            Err(RailwayError::Rejected(
                "the service is already spun down in this environment".to_owned(),
            ))
        });
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty(
                "/services/service-parked/spin-down?environment=env-1",
                Some(&cookie),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body["error"]["message"],
            "the service is already spun down in this environment"
        );
    }

    #[tokio::test]
    async fn the_spin_up_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(
            &app,
            testing::post_empty("/services/service-1/spin-up?environment=env-1", None),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn a_spin_up_without_an_environment_is_rejected() {
        let mut railway = MockRailwayApi::new();
        railway.expect_spin_up().never();
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty("/services/service-1/spin-up", Some(&cookie)),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn spinning_up_a_service_missing_from_an_environment_is_not_found() {
        let mut railway = MockRailwayApi::new();
        railway
            .expect_spin_up()
            .returning(|_, service_id, environment_id| {
                Err(RailwayError::NotFound(format!(
                    "service `{service_id}` has no instance in `{environment_id}`"
                )))
            });
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty(
                "/services/service-1/spin-up?environment=env-empty",
                Some(&cookie),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    /// Nothing spun down is the caller's situation, answered with Railway's
    /// own message — not a `503` pretending the provider is down.
    #[tokio::test]
    async fn spinning_up_a_running_service_is_rejected() {
        let mut railway = MockRailwayApi::new();
        railway.expect_spin_up().returning(|_, _, _| {
            Err(RailwayError::Rejected(
                "the service is not spun down in this environment".to_owned(),
            ))
        });
        let (app, cookie) = app_with_railway(railway);

        let (status, body) = testing::send(
            &app,
            testing::post_empty(
                "/services/service-running/spin-up?environment=env-1",
                Some(&cookie),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body["error"]["message"],
            "the service is not spun down in this environment"
        );
    }
}
