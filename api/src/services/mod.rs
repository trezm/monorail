//! Business logic that sits behind the HTTP layer.
//!
//! Each module owns one capability and exposes it as a trait, so handlers
//! depend on the behaviour rather than on a concrete backend.

pub mod auth;
pub mod autoscaling;
pub mod container;
pub mod jwks;
pub mod railway;
pub mod session;
