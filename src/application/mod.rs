//! Application layer — use cases, services, and port definitions.
//!
//! This layer sits between the domain and infrastructure. It defines
//! port traits (interfaces) that the infrastructure must implement,
//! and houses the service logic that orchestrates domain operations.

pub mod ports;
