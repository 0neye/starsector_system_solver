//! Locate the Starsector installation on disk.
//!
//! Resolution order: explicit `--starsector-dir` flag, then the
//! `STARSECTOR_DIR` environment variable, then a scan of well-known
//! install locations.

use std::path::{Path, PathBuf};

use crate::extract::{ExtractError, Result};

/// Environment variable consulted when no explicit install dir is given.
pub const STARSECTOR_DIR_ENV: &str = "STARSECTOR_DIR";

/// A directory counts as a Starsector install if it contains the
/// `starsector-core` subdirectory (that is what `gamedata.rs` reads).
pub fn is_starsector_dir(path: &Path) -> bool {
    path.join("starsector-core").is_dir()
}

/// The saves directory of an install: `<starsector_dir>/saves`.
pub fn default_saves_dir(starsector_dir: &Path) -> PathBuf {
    starsector_dir.join("saves")
}

/// Try to find a Starsector install: `STARSECTOR_DIR` env var first, then
/// well-known install locations. Only returns directories that validate
/// per [`is_starsector_dir`]. Returns `None` if the env var is set but
/// invalid (use [`resolve_starsector_dir`] to get an error message instead).
pub fn detect_starsector_dir() -> Option<PathBuf> {
    if let Some(dir) = env_starsector_dir() {
        if is_starsector_dir(&dir) {
            return Some(dir);
        }
        return None;
    }
    detect_well_known()
}

/// Resolve the Starsector install dir for the CLI. An explicit flag wins
/// (and must validate); otherwise the `STARSECTOR_DIR` env var (must
/// validate if set); otherwise well-known install locations.
pub fn resolve_starsector_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    resolve_with(explicit, env_starsector_dir(), detect_well_known)
}

fn env_starsector_dir() -> Option<PathBuf> {
    std::env::var_os(STARSECTOR_DIR_ENV).map(PathBuf::from)
}

fn resolve_with(
    explicit: Option<&Path>,
    env_value: Option<PathBuf>,
    detect: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if is_starsector_dir(path) {
            return Ok(path.to_path_buf());
        }
        return Err(ExtractError::NotFound(format!(
            "{} is not a Starsector install (expected a `starsector-core` subdirectory)",
            path.display()
        )));
    }

    if let Some(path) = env_value {
        if is_starsector_dir(&path) {
            return Ok(path);
        }
        return Err(ExtractError::NotFound(format!(
            "{STARSECTOR_DIR_ENV} is set to {} but it is not a Starsector install \
             (expected a `starsector-core` subdirectory)",
            path.display()
        )));
    }

    detect().ok_or_else(|| {
        ExtractError::NotFound(format!(
            "could not find a Starsector install; pass --starsector-dir <path> \
             or set the {STARSECTOR_DIR_ENV} environment variable"
        ))
    })
}

/// Scan well-known install locations for the current platform.
fn detect_well_known() -> Option<PathBuf> {
    well_known_candidates()
        .into_iter()
        .find(|path| is_starsector_dir(path))
}

fn well_known_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", letter as char));
            if !root.exists() {
                continue;
            }
            candidates.push(root.join("Program Files (x86)\\Fractal Softworks\\Starsector"));
            candidates.push(root.join("Program Files\\Fractal Softworks\\Starsector"));
            candidates.push(root.join("Games\\Starsector"));
            candidates.push(root.join("Starsector"));
        }
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/Starsector.app"));
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            candidates.push(home.join("starsector"));
            candidates.push(home.join("games/starsector"));
        }
        candidates.push(PathBuf::from("/opt/starsector"));
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Temp dir that is removed on drop. Avoids depending on a real install
    /// and avoids mutating process env (tests run in parallel).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "system_solver_locate_test_{}_{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fake_install() -> TempDir {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path().join("starsector-core")).unwrap();
        dir
    }

    #[test]
    fn is_starsector_dir_requires_core_subdir() {
        let install = fake_install();
        assert!(is_starsector_dir(install.path()));

        let not_install = TempDir::new();
        assert!(!is_starsector_dir(not_install.path()));
        assert!(!is_starsector_dir(Path::new(
            "definitely/not/a/real/path/anywhere"
        )));
    }

    #[test]
    fn default_saves_dir_appends_saves() {
        assert_eq!(
            default_saves_dir(Path::new("install")),
            Path::new("install").join("saves")
        );
    }

    #[test]
    fn resolve_prefers_valid_explicit_flag() {
        let install = fake_install();
        let resolved = resolve_with(Some(install.path()), None, || None).unwrap();
        assert_eq!(resolved, install.path());
    }

    #[test]
    fn resolve_rejects_invalid_explicit_flag() {
        let not_install = TempDir::new();
        // Even with a valid env var and detection available, an invalid
        // explicit flag is an error, not a silent fallback.
        let install = fake_install();
        let env_value = Some(install.path().to_path_buf());
        let err = resolve_with(Some(not_install.path()), env_value, || {
            Some(install.path().to_path_buf())
        })
        .unwrap_err();
        assert!(matches!(err, ExtractError::NotFound(_)));
        assert!(err.to_string().contains("starsector-core"));
    }

    #[test]
    fn resolve_uses_valid_env_var() {
        let install = fake_install();
        let resolved = resolve_with(None, Some(install.path().to_path_buf()), || None).unwrap();
        assert_eq!(resolved, install.path());
    }

    #[test]
    fn resolve_errors_on_invalid_env_var_without_falling_back() {
        let not_install = TempDir::new();
        let install = fake_install();
        let err = resolve_with(None, Some(not_install.path().to_path_buf()), || {
            Some(install.path().to_path_buf())
        })
        .unwrap_err();
        assert!(err.to_string().contains(STARSECTOR_DIR_ENV));
    }

    #[test]
    fn resolve_falls_back_to_detection() {
        let install = fake_install();
        let detected = install.path().to_path_buf();
        let resolved = resolve_with(None, None, move || Some(detected)).unwrap();
        assert_eq!(resolved, install.path());
    }

    #[test]
    fn resolve_errors_with_actionable_message_when_nothing_found() {
        let err = resolve_with(None, None, || None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--starsector-dir"));
        assert!(message.contains(STARSECTOR_DIR_ENV));
    }
}
