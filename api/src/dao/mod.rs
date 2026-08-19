//! Data access. One module per table, holding its models and — where the table
//! is written or read on its own — a trait.
//!
//! This is the layer that owns diesel. Nothing above it names a table, a column
//! or a `schema::` path, which is what lets a service — and through it a route —
//! be tested against a mock instead of a database.
//!
//! The split against [`services`](crate::services) is by kind of knowledge
//! rather than by size: a DAO knows how rows are stored and nothing about why,
//! a service knows the rule and nothing about where the rows live. So
//! [`sessions::SessionDao`] will write a session with any expiry it is handed,
//! and it is [`SessionStore`](crate::services::session::SessionStore) that
//! decides the expiry is `now + ttl`.
//!
//! A trait method is the transaction boundary. Each one takes its own pooled
//! connection, so two calls cannot be one transaction — a write spanning tables
//! is therefore one method, which is why the login writes `users` and
//! `sessions` through [`sessions::SessionDao::open_login`] and [`users`] holds
//! no trait at all.
//!
//! Errors stay [`DbError`](crate::db::DbError): a DAO has no opinion on what a
//! failed query means, and the service above it does.

pub mod sessions;
pub mod users;
