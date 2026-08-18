//! Clients for external systems.
//!
//! Each module wraps one third-party service and implements whichever
//! capability trait from [`crate::services`] that service can satisfy. Keeping
//! them here, rather than beside the trait, means the trait never grows a
//! dependency on any particular vendor.

pub mod railway;
