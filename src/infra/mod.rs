//! Cross-cutting infrastructure: CLI args, DB pool + backup, logging, decimal
//! helpers, and the maintenance-job scheduler. No domain logic lives here.
pub mod args;
pub mod db;
pub mod decimal;
pub mod logging;
pub mod scheduler;
