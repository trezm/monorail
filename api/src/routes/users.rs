//! The signed-in user.

use axum::{Router, routing::get};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    dao::users::User,
    extract::{CurrentUser, Json},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/users/me", get(me))
}

/// What `/api/v1/users/me` returns.
///
/// Deliberately not the [`User`] row: a response shape that grows a column
/// every time the table does is how internals leak.
#[derive(Debug, Serialize)]
pub struct Profile {
    pub id: Uuid,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<User> for Profile {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            avatar_url: user.avatar_url,
        }
    }
}

async fn me(CurrentUser(user): CurrentUser) -> Json<Profile> {
    Json(user.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use chrono::{TimeDelta, Utc};
    use uuid::Uuid;

    use crate::{
        dao::users::User,
        routes::auth::SESSION_COOKIE,
        secret::Secret,
        services::{
            auth::TokenSet,
            session::{MockSessionStore, Session},
        },
        testing,
    };

    fn session() -> Session {
        Session {
            id: Uuid::nil(),
            user: User {
                id: Uuid::nil(),
                railway_user_id: "user_stub".to_owned(),
                email: Some("jane@example.test".to_owned()),
                name: Some("Jane Developer".to_owned()),
                avatar_url: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            tokens: TokenSet {
                access_token: Secret::new("access-stub"),
                refresh_token: None,
                id_token: None,
                scope: "openid email".to_owned(),
                expires_at: Utc::now() + TimeDelta::seconds(3600),
            },
            expires_at: Utc::now() + TimeDelta::seconds(3600),
        }
    }

    fn app_with(sessions: MockSessionStore) -> axum::Router {
        testing::app(testing::state().with_sessions(Arc::new(sessions)))
    }

    /// No cookie means the store is never asked — the extractor rejects first.
    #[tokio::test]
    async fn the_profile_endpoint_requires_a_session() {
        let mut sessions = MockSessionStore::new();
        sessions.expect_lookup().never();

        let (status, body) =
            testing::send(&app_with(sessions), testing::get("/api/v1/users/me")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    /// A cookie naming no session is the same `401` as no cookie at all: which
    /// of the two it was is not a caller's business.
    #[tokio::test]
    async fn an_unknown_session_cookie_is_not_a_login() {
        let mut sessions = MockSessionStore::new();
        sessions
            .expect_lookup()
            .withf(|token| token.expose() == "made-up")
            .times(1)
            .returning(|_| Ok(None));

        let (status, body) = testing::send(
            &app_with(sessions),
            testing::get_with_cookie("/api/v1/users/me", &format!("{SESSION_COOKIE}=made-up")),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn a_logged_in_browser_reads_its_own_profile() {
        let mut sessions = MockSessionStore::new();
        sessions
            .expect_lookup()
            .withf(|token| token.expose() == "opaque-token")
            .times(1)
            .returning(|_| Ok(Some(session())));

        let (status, body) = testing::send(
            &app_with(sessions),
            testing::get_with_cookie(
                "/api/v1/users/me",
                &format!("{SESSION_COOKIE}=opaque-token"),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["email"], "jane@example.test");
        assert_eq!(body["name"], "Jane Developer");
        assert!(
            body.get("railway_user_id").is_none(),
            "the profile should not mirror the row"
        );
        assert!(
            body.get("created_at").is_none(),
            "the profile should not mirror the row"
        );
    }
}
