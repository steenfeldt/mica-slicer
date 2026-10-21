// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
// Source (original): Mica job boundary and cache lifecycle.
// Changes: Run CLI/GUI jobs in a scoped worker pool with a per-job Clipper cache.

//! Panic containment for CLI/GUI jobs. Ordinary errors are preserved; unwind panics get a report.
//! Allocation failure/abort/stack overflow cannot be recovered by Rust's catch_unwind.
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct Boundary {
    stage: Mutex<String>,
}
impl Boundary {
    pub fn new(stage: &str) -> Self {
        Self {
            stage: Mutex::new(stage.into()),
        }
    }
    pub fn stage(&self, stage: &str) {
        *self.stage.lock().unwrap_or_else(|p| p.into_inner()) = stage.into();
    }
    pub fn run<T: Send>(
        &self,
        work: impl FnOnce() -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.run_with_log_dir(&log_directory(), work)
    }
    pub fn run_with_log_dir<T: Send>(
        &self,
        dir: &Path,
        work: impl FnOnce() -> Result<T, String> + Send,
    ) -> Result<T, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::geometry::clip_cache::with_job(work)?
        })) {
            Ok(result) => result,
            Err(payload) => {
                let stage = self.stage.lock().unwrap_or_else(|p| p.into_inner()).clone();
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-text panic");
                let hint = "Check model integrity and profile overrides, then retry. The scene is preserved; include the crash log when reporting this issue.";
                let time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let path = dir.join(format!("crash-{}-{time}.log", std::process::id()));
                let report = format!(
                    "Mica {}\nStage: {stage}\nPanic: {message}\n{hint}\nPost-unwind boundary backtrace (not the original panic stack):\n{}\n",
                    crate::VERSION,
                    std::backtrace::Backtrace::force_capture()
                );
                let written =
                    std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, report));
                let log = match written {
                    Ok(()) => format!("Crash log: {}", path.display()),
                    Err(e) => format!("Could not write crash log at {}: {e}", path.display()),
                };
                Err(format!(
                    "Unexpected failure during {stage}: {message}. {hint} {log}"
                ))
            }
        }
    }
}
/// Where crash logs go. `MICA_CRASH_LOG_DIR` overrides the platform folder (test harnesses
/// such as the GUI monkey test keep their logs out of the user's).
pub fn log_directory() -> PathBuf {
    crate::user_data::crash_log_dir()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caught_panic_preserves_stage_and_writes_log() {
        let dir = std::env::temp_dir().join(format!("mica-crash-test-{}", std::process::id()));
        let job = Boundary::new("load");
        let result: Result<(), String> = job.run_with_log_dir(&dir, || {
            job.stage("supports");
            panic!("test input");
        });
        assert!(result.unwrap_err().contains("during supports"));
        let path = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("Panic: test input")
        );
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(job.run(|| Ok(42)), Ok(42));
    }
}
