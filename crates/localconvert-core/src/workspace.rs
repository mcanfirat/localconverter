//! Per-job temporary workspaces.
//!
//! Layout, exactly as specified:
//!
//! ```text
//! <app-temp>/jobs/<job-id>/
//! ```
//!
//! Every job gets its own directory, staged output is written there, and the
//! validated result is committed to the user's destination from there. Cleanup
//! runs on success, failure, cancellation *and* at next startup, and is fenced
//! so that it can only ever delete inside the LocalConvert temp root.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{ConversionError, ConversionErrorCode, Result};
use crate::paths;

/// `<app-temp>/jobs`
#[must_use]
pub fn jobs_root(app_temp_root: &Path) -> PathBuf {
    app_temp_root.join("jobs")
}

/// An owned temporary directory for one job. Removed on drop.
#[derive(Debug)]
pub struct JobWorkspace {
    job_id: Uuid,
    root: PathBuf,
    /// Canonical `<app-temp>/jobs`, captured at creation. Cleanup refuses to
    /// delete anything that does not resolve inside it.
    fence: PathBuf,
}

impl JobWorkspace {
    /// Creates `<app-temp>/jobs/<job-id>/`.
    pub fn create(app_temp_root: &Path, job_id: Uuid) -> Result<Self> {
        let jobs = jobs_root(app_temp_root);
        std::fs::create_dir_all(&jobs)
            .map_err(|err| ConversionError::from_io("create jobs temp root", &err))?;
        let fence = jobs
            .canonicalize()
            .map_err(|err| ConversionError::from_io("canonicalize jobs temp root", &err))?;

        let root = fence.join(job_id.to_string());
        std::fs::create_dir(&root)
            .map_err(|err| ConversionError::from_io("create job workspace", &err))?;

        Ok(Self {
            job_id,
            root,
            fence,
        })
    }

    #[must_use]
    pub fn job_id(&self) -> Uuid {
        self.job_id
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Resolves a staging path inside the workspace. Rejects traversal.
    pub fn child(&self, relative: &str) -> Result<PathBuf> {
        paths::join_within(&self.root, Path::new(relative))
    }

    /// Partial outputs carry this suffix until they have been validated, so a
    /// crash can never leave something that looks like a finished conversion.
    pub fn staging_path(&self, file_name: &str) -> Result<PathBuf> {
        self.child(&format!("{file_name}.partial"))
    }

    /// Removes the workspace now instead of at drop, surfacing errors.
    pub fn remove(&self) -> Result<()> {
        remove_fenced(&self.fence, &self.root)
    }
}

impl Drop for JobWorkspace {
    fn drop(&mut self) {
        if let Err(err) = remove_fenced(&self.fence, &self.root) {
            // A leaked temp directory is recovered by cleanup_stale at next
            // startup, so this is a warning rather than a hard failure.
            tracing::warn!(job_id = %self.job_id, error = %err, "failed to remove job workspace");
        }
    }
}

/// Deletes `target` only if it canonically resolves inside `fence`.
fn remove_fenced(fence: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    if !paths::is_within(fence, target)? {
        return Err(ConversionError::new(
            ConversionErrorCode::InternalError,
            "refusing to remove a path outside the LocalConvert temp root",
        ));
    }
    std::fs::remove_dir_all(target)
        .map_err(|err| ConversionError::from_io("remove job workspace", &err))
}

/// Startup recovery: removes job directories left behind by a crash.
///
/// Only entries whose name parses as a UUID are considered, so a directory that
/// is not ours is never touched even if it somehow ends up under `jobs/`.
/// Returns the number of directories removed.
pub fn cleanup_stale(app_temp_root: &Path, active: &HashSet<Uuid>) -> Result<usize> {
    let jobs = jobs_root(app_temp_root);
    if !jobs.exists() {
        return Ok(0);
    }
    let fence = jobs
        .canonicalize()
        .map_err(|err| ConversionError::from_io("canonicalize jobs temp root", &err))?;

    let entries = std::fs::read_dir(&fence)
        .map_err(|err| ConversionError::from_io("read jobs temp root", &err))?;

    let mut removed = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(error = %err, "skipping unreadable temp entry");
                continue;
            }
        };

        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(job_id) = Uuid::parse_str(name) else {
            tracing::debug!(entry = %name, "leaving non-LocalConvert entry in temp root");
            continue;
        };
        if active.contains(&job_id) {
            continue;
        }

        // A symlink here would make file_type() report a link, never a dir, and
        // remove_fenced would refuse it anyway once it resolves outside.
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {}
            Ok(_) => continue,
            Err(err) => {
                tracing::warn!(error = %err, "skipping temp entry with unreadable type");
                continue;
            }
        }

        match remove_fenced(&fence, &entry.path()) {
            Ok(()) => {
                removed += 1;
                tracing::info!(job_id = %job_id, "removed stale job workspace");
            }
            Err(err) => tracing::warn!(job_id = %job_id, error = %err, "stale cleanup failed"),
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn create_makes_the_specified_layout() {
        let temp = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let ws = JobWorkspace::create(temp.path(), id).unwrap();

        assert!(ws.path().is_dir());
        assert_eq!(
            ws.path().file_name().unwrap().to_string_lossy(),
            id.to_string()
        );
        assert_eq!(ws.path().parent().unwrap().file_name().unwrap(), "jobs");
    }

    #[test]
    fn drop_removes_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let path = {
            let ws = JobWorkspace::create(temp.path(), Uuid::new_v4()).unwrap();
            std::fs::write(ws.child("staged.bin").unwrap(), b"partial").unwrap();
            ws.path().to_path_buf()
        };
        assert!(!path.exists(), "workspace must not survive its owner");
    }

    #[test]
    fn child_rejects_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let ws = JobWorkspace::create(temp.path(), Uuid::new_v4()).unwrap();

        assert!(ws.child("out.jpg").is_ok());
        assert!(ws.child("../../escape.jpg").is_err());
        assert!(ws.child("/etc/passwd").is_err());
    }

    #[test]
    fn staging_paths_are_marked_partial() {
        let temp = tempfile::tempdir().unwrap();
        let ws = JobWorkspace::create(temp.path(), Uuid::new_v4()).unwrap();
        let staged = ws.staging_path("photo.jpg").unwrap();
        assert_eq!(staged.file_name().unwrap(), "photo.jpg.partial");
    }

    #[test]
    fn cleanup_stale_removes_only_inactive_job_directories() {
        let temp = tempfile::tempdir().unwrap();
        let stale = Uuid::new_v4();
        let active_id = Uuid::new_v4();

        let jobs = jobs_root(temp.path());
        std::fs::create_dir_all(jobs.join(stale.to_string())).unwrap();
        std::fs::create_dir_all(jobs.join(active_id.to_string())).unwrap();

        let active: HashSet<Uuid> = std::iter::once(active_id).collect();
        assert_eq!(cleanup_stale(temp.path(), &active).unwrap(), 1);

        assert!(!jobs.join(stale.to_string()).exists());
        assert!(jobs.join(active_id.to_string()).exists());
    }

    #[test]
    fn cleanup_stale_ignores_entries_that_are_not_ours() {
        let temp = tempfile::tempdir().unwrap();
        let jobs = jobs_root(temp.path());
        std::fs::create_dir_all(jobs.join("not-a-uuid")).unwrap();
        std::fs::write(jobs.join("stray.log"), b"log").unwrap();

        assert_eq!(cleanup_stale(temp.path(), &HashSet::new()).unwrap(), 0);
        assert!(jobs.join("not-a-uuid").exists());
        assert!(jobs.join("stray.log").exists());
    }

    #[test]
    fn cleanup_stale_on_a_missing_root_is_a_no_op() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(cleanup_stale(temp.path(), &HashSet::new()).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_never_follows_a_symlink_out_of_the_temp_root() {
        let temp = tempfile::tempdir().unwrap();
        let precious = temp.path().join("precious");
        std::fs::create_dir_all(&precious).unwrap();
        std::fs::write(precious.join("originals.heic"), b"user data").unwrap();

        let jobs = jobs_root(temp.path());
        std::fs::create_dir_all(&jobs).unwrap();
        let disguised = Uuid::new_v4().to_string();
        std::os::unix::fs::symlink(&precious, jobs.join(&disguised)).unwrap();

        assert_eq!(cleanup_stale(temp.path(), &HashSet::new()).unwrap(), 0);
        assert!(
            precious.join("originals.heic").exists(),
            "cleanup must never delete through a symlink"
        );
    }

    #[test]
    fn remove_fenced_refuses_targets_outside_the_fence() {
        let temp = tempfile::tempdir().unwrap();
        let fence = temp.path().join("jobs");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&fence).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        assert!(remove_fenced(&fence, &outside).is_err());
        assert!(outside.exists());
    }
}
