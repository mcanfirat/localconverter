//! Safe path handling.
//!
//! Every filesystem primitive in LocalConvert routes through this module. The
//! guarantees it exists to provide:
//!
//! * a path containing a NUL byte never reaches a child process argv;
//! * "is this path inside that root" is answered on *canonical* paths, so
//!   `..` segments and symlinks cannot escape (archive extraction, temp cleanup);
//! * an existing file is never overwritten without an explicit decision;
//! * output lands at its destination atomically, so a crash mid-write cannot
//!   leave a truncated file that looks like a successful conversion.

use std::path::{Component, Path, PathBuf};

use crate::error::{ConversionError, ConversionErrorCode, Result};

/// Rejects paths that cannot be safely handed to an OS API or a child process.
///
/// A NUL byte truncates the path at the C boundary, so `evil.jpg\0.png` opens
/// `evil.jpg` while every check upstream believed it was a PNG.
pub fn ensure_safe_component_bytes(path: &Path) -> Result<()> {
    let has_nul = {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes().contains(&0)
        }
        #[cfg(not(unix))]
        {
            path.as_os_str().encode_wide().any(|unit| unit == 0)
        }
    };
    if has_nul {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "path contains a NUL byte",
        )
        .with_message_key("error.path.nulByte"));
    }
    Ok(())
}

#[cfg(not(unix))]
use std::os::windows::ffi::OsStrExt;

/// Canonicalizes as much of `path` as exists on disk, then re-appends the
/// remaining components.
///
/// `Path::canonicalize` fails outright when the target does not exist, but the
/// paths we most need to security-check are precisely the ones we are about to
/// create (an extraction destination, a job output). Walking up to the deepest
/// existing ancestor resolves every symlink that actually exists, which is what
/// an escape attempt has to go through.
///
/// `..` in the not-yet-existing remainder is rejected rather than normalised
/// lexically: normalising it would be wrong in the presence of a symlink that
/// gets created between the check and the write.
pub fn canonicalize_deepest_existing(path: &Path) -> Result<PathBuf> {
    ensure_safe_component_bytes(path)?;

    let mut existing = path.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();

    loop {
        match existing.canonicalize() {
            Ok(canonical) => {
                let mut resolved = canonical;
                for part in remainder.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(std::ffi::OsStr::to_os_string) else {
                    // Ran out of ancestors without finding anything real.
                    return Err(ConversionError::new(
                        ConversionErrorCode::DestinationUnavailable,
                        "no existing ancestor directory for path",
                    ));
                };
                if name == ".." || name == "." {
                    return Err(ConversionError::new(
                        ConversionErrorCode::InvalidInput,
                        "path traverses through a non-existent directory",
                    )
                    .with_message_key("error.path.traversal"));
                }
                remainder.push(name);
                if !existing.pop() {
                    return Err(ConversionError::new(
                        ConversionErrorCode::DestinationUnavailable,
                        "no existing ancestor directory for path",
                    ));
                }
            }
            Err(err) => return Err(ConversionError::from_io("canonicalize", &err)),
        }
    }
}

/// True when `candidate` resolves to a location inside `root`.
///
/// Both sides are canonicalized first, so `root/../../etc/passwd`, a symlinked
/// entry and a `..` component all answer `false`. `root` must exist.
pub fn is_within(root: &Path, candidate: &Path) -> Result<bool> {
    let root = root
        .canonicalize()
        .map_err(|err| ConversionError::from_io("canonicalize root", &err))?;
    let candidate = canonicalize_deepest_existing(candidate)?;
    Ok(candidate.starts_with(&root))
}

/// Joins a caller-supplied relative name onto `root`, refusing anything that
/// could escape. Used for temp-workspace children now and archive entries later.
pub fn join_within(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure_safe_component_bytes(relative)?;

    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    "relative path contains a parent-directory component",
                )
                .with_message_key("error.path.traversal"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ConversionError::new(
                    ConversionErrorCode::InvalidInput,
                    "expected a relative path, got an absolute one",
                )
                .with_message_key("error.path.absolute"));
            }
        }
    }

    let joined = root.join(relative);
    if !is_within(root, &joined)? {
        return Err(ConversionError::new(
            ConversionErrorCode::InvalidInput,
            "resolved path escapes its root",
        )
        .with_message_key("error.path.traversal"));
    }
    Ok(joined)
}

/// What to do when the destination file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OverwritePolicy {
    /// Fail the job rather than touch the existing file. The safe default.
    Fail,
    /// Pick the next free `name (n).ext`.
    Rename,
    /// Replace it. Only ever set from an explicit user confirmation.
    Overwrite,
    /// Report the job as skipped without writing.
    Skip,
}

/// Resolves `dir/file_name` against `policy`, returning `Ok(None)` for `Skip`.
///
/// Naming follows the spec exactly: `photo.jpg`, `photo (1).jpg`, `photo (2).jpg`.
pub fn resolve_destination(
    dir: &Path,
    file_name: &str,
    policy: OverwritePolicy,
) -> Result<Option<PathBuf>> {
    let candidate = join_within(dir, Path::new(file_name))?;
    if !candidate.exists() {
        return Ok(Some(candidate));
    }

    match policy {
        OverwritePolicy::Overwrite => Ok(Some(candidate)),
        OverwritePolicy::Skip => Ok(None),
        OverwritePolicy::Fail => Err(ConversionError::new(
            ConversionErrorCode::DestinationUnavailable,
            format!("destination already exists: {file_name}"),
        )
        .with_message_key("error.destination.exists")),
        OverwritePolicy::Rename => {
            let (stem, extension) = split_file_name(file_name);
            // Bounded: never spin forever on a pathological directory.
            for n in 1..=9_999u32 {
                let next = match extension {
                    Some(ext) => format!("{stem} ({n}).{ext}"),
                    None => format!("{stem} ({n})"),
                };
                let path = join_within(dir, Path::new(&next))?;
                if !path.exists() {
                    return Ok(Some(path));
                }
            }
            Err(ConversionError::new(
                ConversionErrorCode::DestinationUnavailable,
                "exhausted automatic rename attempts",
            )
            .with_message_key("error.destination.renameExhausted"))
        }
    }
}

/// Splits on the *last* dot so `archive.tar.gz` renames to `archive.tar (1).gz`
/// rather than `archive (1).tar.gz`. Leading-dot files (`.gitignore`) are
/// treated as having no extension.
fn split_file_name(file_name: &str) -> (&str, Option<&str>) {
    match file_name.rfind('.') {
        Some(index) if index > 0 => {
            let (stem, rest) = file_name.split_at(index);
            (stem, rest.strip_prefix('.').filter(|ext| !ext.is_empty()))
        }
        _ => (file_name, None),
    }
}

/// Moves a completed, already-validated file to its final destination.
///
/// Renames when possible (atomic within a filesystem); falls back to
/// copy-then-remove across devices, which is the case when the app temp
/// directory and the user's destination live on different volumes.
///
/// The existence check is deliberately *before* the move and deliberately not
/// atomic — see the ponytail note below.
pub fn commit_output(from: &Path, to: &Path, allow_overwrite: bool) -> Result<()> {
    ensure_safe_component_bytes(from)?;
    ensure_safe_component_bytes(to)?;

    if !allow_overwrite && to.exists() {
        return Err(ConversionError::new(
            ConversionErrorCode::DestinationUnavailable,
            format!("refusing to overwrite {}", crate::error::display_name(to)),
        )
        .with_message_key("error.destination.exists"));
    }

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ConversionError::from_io("create destination directory", &err))?;
    }

    // ponytail: TOCTOU window between exists() and rename(). On a single-user
    // desktop the loser is the user's own second job, and the queue serialises
    // those. Upgrade to O_EXCL create + rename if LocalConvert ever grows a
    // headless multi-process mode.
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)
                .map_err(|err| ConversionError::from_io("copy output to destination", &err))?;
            std::fs::remove_file(from)
                .map_err(|err| ConversionError::from_io("remove staged output", &err))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn nul_bytes_are_rejected() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = Path::new(std::ffi::OsStr::from_bytes(b"evil.jpg\0.png"));
            assert!(ensure_safe_component_bytes(path).is_err());
        }
        assert!(ensure_safe_component_bytes(Path::new("fine.jpg")).is_ok());
    }

    #[test]
    fn is_within_accepts_children_and_rejects_parents() {
        let dir = tmp();
        let root = dir.path();
        let child = root.join("sub/output.jpg");
        std::fs::create_dir_all(root.join("sub")).unwrap();

        assert!(is_within(root, &child).unwrap());
        assert!(!is_within(root, root.parent().unwrap()).unwrap());
    }

    #[test]
    fn is_within_rejects_dotdot_escape() {
        let dir = tmp();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(dir.path().join("outside")).unwrap();

        let escape = root.join("../outside");
        assert!(!is_within(&root, &escape).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn is_within_rejects_symlink_escape() {
        let dir = tmp();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // The link itself and anything under it resolve outside the root.
        assert!(!is_within(&root, &root.join("link")).unwrap());
        assert!(!is_within(&root, &root.join("link/loot.txt")).unwrap());
    }

    #[test]
    fn join_within_rejects_traversal_and_absolute_paths() {
        let dir = tmp();
        let root = dir.path();

        assert!(join_within(root, Path::new("a/b/c.txt")).is_ok());
        assert!(join_within(root, Path::new("../escape.txt")).is_err());
        assert!(join_within(root, Path::new("a/../../escape.txt")).is_err());
        assert!(join_within(root, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn join_within_preserves_unicode_and_spaces() {
        let dir = tmp();
        let root = dir.path();
        for name in ["ünïcode tëst.jpg", "my photo.png", "写真 🎉.heic"] {
            let joined = join_within(root, Path::new(name)).unwrap();
            assert_eq!(joined.file_name().unwrap().to_string_lossy(), name);
        }
    }

    #[test]
    fn canonicalize_deepest_existing_handles_missing_leaf() {
        let dir = tmp();
        let target = dir.path().join("not-yet/created.jpg");
        let resolved = canonicalize_deepest_existing(&target).unwrap();
        assert!(resolved.ends_with("not-yet/created.jpg"));
        assert!(resolved.is_absolute());
    }

    #[test]
    fn rename_policy_produces_spec_naming() {
        let dir = tmp();
        let root = dir.path();
        std::fs::write(root.join("photo.jpg"), b"x").unwrap();

        let first = resolve_destination(root, "photo.jpg", OverwritePolicy::Rename)
            .unwrap()
            .unwrap();
        assert_eq!(first.file_name().unwrap(), "photo (1).jpg");

        std::fs::write(&first, b"x").unwrap();
        let second = resolve_destination(root, "photo.jpg", OverwritePolicy::Rename)
            .unwrap()
            .unwrap();
        assert_eq!(second.file_name().unwrap(), "photo (2).jpg");
    }

    #[test]
    fn fail_policy_refuses_and_skip_policy_yields_none() {
        let dir = tmp();
        let root = dir.path();
        std::fs::write(root.join("photo.jpg"), b"x").unwrap();

        assert!(resolve_destination(root, "photo.jpg", OverwritePolicy::Fail).is_err());
        assert!(
            resolve_destination(root, "photo.jpg", OverwritePolicy::Skip)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_destination(root, "photo.jpg", OverwritePolicy::Overwrite)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn extension_split_keeps_multi_part_extensions_sane() {
        assert_eq!(split_file_name("photo.jpg"), ("photo", Some("jpg")));
        assert_eq!(
            split_file_name("archive.tar.gz"),
            ("archive.tar", Some("gz"))
        );
        assert_eq!(split_file_name("README"), ("README", None));
        assert_eq!(split_file_name(".gitignore"), (".gitignore", None));
        assert_eq!(split_file_name("trailing."), ("trailing", None));
    }

    #[test]
    fn commit_output_refuses_to_overwrite_unless_allowed() {
        let dir = tmp();
        let staged = dir.path().join("staged.bin");
        let dest = dir.path().join("dest.bin");
        std::fs::write(&staged, b"new").unwrap();
        std::fs::write(&dest, b"original").unwrap();

        let err = commit_output(&staged, &dest, false).unwrap_err();
        assert_eq!(err.code, ConversionErrorCode::DestinationUnavailable);
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
        assert!(
            staged.exists(),
            "staged output must survive a refused commit"
        );

        commit_output(&staged, &dest, true).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!staged.exists());
    }

    #[test]
    fn commit_output_creates_missing_destination_directories() {
        let dir = tmp();
        let staged = dir.path().join("staged.bin");
        let dest = dir.path().join("a/b/c/out.bin");
        std::fs::write(&staged, b"data").unwrap();

        commit_output(&staged, &dest, false).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    }
}
