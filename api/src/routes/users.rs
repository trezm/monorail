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
