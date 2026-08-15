//! HTTP surface. Each module owns one resource and exports a `router()`.

pub mod health;
pub mod widgets;

use axum::Router;

use crate::state::AppState;

/// Everything served under `/api/v1`.
///
/// Nest a new resource here; versioning happens by adding an `api_v2()`
/// alongside this rather than by mutating it.
pub fn api_v1() -> Router<AppState> {
    Router::new().nest("/widgets", widgets::router())
}
