//! Sample CRUD resource: `/api/v1/widgets`.
//!
//! Shows the conventions worth copying — custom extractors so bad input yields
//! the standard error envelope, `201` with a `Location` header, `204` on
//! delete, and a paginated list envelope.

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    extract::{Json, Path, Query},
    state::AppState,
    widget::{NewWidget, Widget, WidgetPatch},
};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        // axum 0.8 uses `{name}` for path captures; the 0.7 `:name` form is gone.
        .route("/{id}", get(fetch).patch(update).delete(remove))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
pub struct Pagination {
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Page<Widget>>> {
    if query.limit == 0 || query.limit > MAX_LIMIT {
        return Err(ApiError::UnprocessableEntity(format!(
            "limit must be between 1 and {MAX_LIMIT}"
        )));
    }

    let (data, total) = state.widgets().list(query.limit, query.offset)?;

    Ok(Json(Page {
        data,
        pagination: Pagination {
            total,
            limit: query.limit,
            offset: query.offset,
        },
    }))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<NewWidget>,
) -> ApiResult<impl IntoResponse> {
    let widget = state.widgets().create(body)?;
    let location = format!("/api/v1/widgets/{}", widget.id);

    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(widget),
    ))
}

async fn fetch(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<Json<Widget>> {
    state.widgets().get(id).map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(patch): Json<WidgetPatch>,
) -> ApiResult<Json<Widget>> {
    state.widgets().update(id, patch).map(Json)
}

async fn remove(State(state): State<AppState>, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    state.widgets().delete(id)?;
    Ok(StatusCode::NO_CONTENT)
}
