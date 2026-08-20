//! The signed-in user.

use axum::{Router, routing::get};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    extract::{CurrentUser, Json},
    services::session::User,
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
    use axum::http::StatusCode;

    use crate::{
        routes::auth::SESSION_COOKIE,
        services::{auth::MockAuthProvider, railway::MockRailwayApi, session::MockSessionStore},
        testing,
    };

    #[tokio::test]
    async fn the_profile_endpoint_requires_a_session() {
        let app = super::router().with_state(testing::untouched_state());

        let (status, body) = testing::send(&app, testing::get("/users/me")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn an_unknown_session_cookie_is_not_a_login() {
        let mut sessions = MockSessionStore::new();
        sessions.expect_lookup().returning(|_| Ok(None));

        let app = super::router().with_state(testing::state(
            MockAuthProvider::new(),
            sessions,
            MockRailwayApi::new(),
        ));

        let (status, _) = testing::send(
            &app,
            testing::get_with_cookie("/users/me", &format!("{SESSION_COOKIE}=made-up")),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
