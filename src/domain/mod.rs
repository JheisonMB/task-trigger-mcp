//! Domain layer — core business entities and validation rules.
//!
//! This is the innermost layer of the architecture. It has no dependencies
//! on infrastructure, frameworks, or external crates beyond basic utilities.

pub mod cli_config;
pub mod cli_strategy;
pub mod models;
pub mod models_db;
pub mod notification;
pub mod validation;
