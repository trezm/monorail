//! The `users` table: one row per Railway account this service has seen.
//!
//! Models only, and no trait of its own. The single write to `users` happens
//! inside the login transaction, so it belongs to
//! [`SessionDao::open_login`](crate::dao::sessions::SessionDao::open_login) —
//! a `UserDao` here would have to take its own pooled connection and could not
//! join that transaction. Add one when something reads a user on its own.

use chrono::{DateTime, Utc};
use diesel::{Identifiable, Queryable, Selectable};
use serde::Serialize;
use uuid::Uuid;

use crate::schema::users;

/// A Railway account, as stored.
///
/// Keyed on the `sub` claim rather than the email address: `sub` is the only
/// claim Railway guarantees, and an email is both absent without the `email`
/// scope and mutable when present.
#[derive(Debug, Clone, PartialEq, Eq, Queryable, Selectable, Identifiable, Serialize)]
#[diesel(table_name = users, check_for_backend(diesel::pg::Pg))]
pub struct User {
    pub id: Uuid,
    pub railway_user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a login knows about an account: everything except the columns the
/// database assigns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub railway_user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}
