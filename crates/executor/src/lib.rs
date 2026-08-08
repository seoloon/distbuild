mod detect;
mod spawn;

pub use detect::{detect_build_system, BuildSystem};
pub use spawn::{run_step, ExecutorError, LogLine, StepResult};
