mod config;
mod process;
mod report;
mod stats;

pub use config::{BenchmarkConfig, OutputFormat, ProcessRole};
pub use process::{ManagedChild, hold_until_stdin_closes};
pub use report::{BenchmarkReport, run_benchmark};
pub use stats::{AggregateSummary, TrialSummary};
