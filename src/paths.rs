//! Per-user filesystem locations for the installed binary, with a
//! working-directory fallback so the in-repo dev workflow keeps working.
//!
//! The DB and config are resolved in this order:
//!   1. an explicit `--db` / `--starsector-dir` flag (handled by clap),
//!   2. the `SYSTEM_SOLVER_DB` environment variable (DB only),
//!   3. an existing file in the per-user data/config directory (the location an
//!      installer writes to),
//!   4. an existing file in the current working directory / legacy `workspace/`
//!      path (the historical dev default), and finally
//!   5. the per-user path as the creation target for a fresh `extract run`.
//!
//! This means an installed user (who has a per-user DB) always finds it
//! regardless of where they launch from, while a developer working from the
//! repo root keeps using `./save_data.db` and `workspace/solver_tui.toml`.

use std::path::PathBuf;

use directories::ProjectDirs;

/// File name used for the extraction DB in every location.
const DB_FILE: &str = "save_data.db";
/// File name used for the TUI/solver config in the per-user config dir.
const CONFIG_FILE: &str = "solver_tui.toml";
/// Legacy in-repo config location used before per-user relocation.
const LEGACY_CONFIG: &str = "workspace/solver_tui.toml";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "StarsectorSystemRanker")
}

/// Per-user data directory (e.g. `%APPDATA%\StarsectorSystemRanker\data`
/// on Windows, `~/.local/share/StarsectorSystemRanker` on Linux). `None` when
/// the platform exposes no home directory.
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.data_dir().to_path_buf())
}

/// Per-user config directory. `None` when the platform exposes no home directory.
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.config_dir().to_path_buf())
}

/// Resolve the default extraction-DB path when no `--db` flag is given.
/// See the module docs for the resolution order.
pub fn default_db_path() -> PathBuf {
    if let Some(env) = std::env::var_os("SYSTEM_SOLVER_DB") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    let user = data_dir().map(|dir| dir.join(DB_FILE));
    if let Some(user) = &user {
        if user.exists() {
            return user.clone();
        }
    }
    let cwd = PathBuf::from(DB_FILE);
    if cwd.exists() {
        return cwd;
    }
    // Nothing exists yet: prefer the per-user location as the creation target,
    // falling back to the cwd path when there is no home directory.
    user.unwrap_or(cwd)
}

/// Resolve the TUI/solver config file path. See the module docs for the order:
/// an existing per-user config wins, then the legacy `workspace/` file, then
/// the per-user path as the write target.
pub fn config_path() -> PathBuf {
    let user = config_dir().map(|dir| dir.join(CONFIG_FILE));
    if let Some(user) = &user {
        if user.exists() {
            return user.clone();
        }
    }
    let legacy = PathBuf::from(LEGACY_CONFIG);
    if legacy.exists() {
        return legacy;
    }
    user.unwrap_or(legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both env-var checks live in one test: they mutate the shared
    // `SYSTEM_SOLVER_DB` process var, so splitting them would race under the
    // parallel test runner.
    #[test]
    fn db_path_resolution() {
        std::env::set_var("SYSTEM_SOLVER_DB", "custom/location.db");
        assert_eq!(default_db_path(), PathBuf::from("custom/location.db"));

        // Without the override the file name is always save_data.db, wherever
        // it resolves.
        std::env::remove_var("SYSTEM_SOLVER_DB");
        assert_eq!(default_db_path().file_name().unwrap(), DB_FILE);
    }

    #[test]
    fn config_path_is_named_consistently() {
        let path = config_path();
        let name = path.file_name().unwrap();
        assert!(name == CONFIG_FILE || name == "solver_tui.toml");
    }
}
