//! Cross-cutting infrastructure: CLI args, DB pool + backup, logging, decimal
//! helpers, AUD FX conversion, the maintenance-job scheduler, and the optional
//! shared-credential access control. No domain logic lives here.
pub mod args;
pub mod auth;
pub mod config;
pub mod date;
pub mod db;
pub mod decimal;
pub mod fetch;
pub mod fx;
pub mod http;
pub mod logging;
pub mod scheduler;
