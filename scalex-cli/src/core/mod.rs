// Core library modules — public APIs for CLI commands and future integrations.
// Some functions are currently only exercised by tests; suppress dead_code warnings
// for functions that are part of the intended public API.
#[allow(dead_code)]
pub mod config;
pub mod error;
pub mod gitops;
pub mod host_prepare;
pub mod kernel;
pub mod kubespray;
pub mod placement;
#[allow(dead_code)]
pub mod resource_planner;
pub mod resource_pool;
#[allow(dead_code)]
pub mod secrets;
pub mod ssh;
pub mod sync;
pub mod tofu;
#[allow(dead_code)]
pub mod validation;
