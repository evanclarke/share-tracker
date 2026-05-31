//! Cross-cutting infrastructure: CLI args, DB pool + backup, logging, decimal
//! helpers, AUD FX conversion, and the maintenance-job scheduler. No domain logic
//! lives here.
pub mod args;
pub mod db;
pub mod decimal;
pub mod fx;
pub mod http;
pub mod logging;
pub mod scheduler;
