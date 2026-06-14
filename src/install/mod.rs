//! Native per-user installer for release archives.
//!
//! This replaces the former `install.py`: the shipped binary installs itself
//! from an unpacked release archive (copy onto `PATH`, run the first
//! extraction, create a launcher, optionally install the bundled agent skill)
//! and can fully reverse it. The shell/PowerShell launchers in the archive are
//! thin shims that just invoke `system_solver install`.
//!
//! Platform specifics (PATH editing, shortcut creation, install location) are
//! split with `#[cfg]`; the pure helpers (PATH string math, skill-target
//! resolution, `.desktop` contents, recursive copy) are unit-tested.

use std::error::Error;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::extract::locate;

/// Vendor/app name used for the per-user install dir and data locations.
const APP_NAME: &str = "StarsectorSystemRanker";
/// Start Menu shortcut file name (Windows).
const START_MENU_NAME: &str = "Starsector System Ranker.lnk";
/// Desktop entry file name (Linux).
const DESKTOP_FILE: &str = "starsector-system-ranker.desktop";
/// Bundled agent skill directory name (under `skills/` in the archive).
const SKILL_NAME: &str = "system-solver";

/// Options for [`run_install`], mirroring the former `install.py` flags.
pub struct InstallOpts {
    pub starsector_dir: Option<PathBuf>,
    pub no_shortcut: bool,
    pub skip_extract: bool,
    pub with_skills: bool,
    pub no_skills: bool,
    pub yes: bool,
}

type Fallible = Result<(), Box<dyn Error>>;

/// True when both stdin and stdout are attached to a terminal, i.e. a person is
/// driving this directly (as opposed to a script or piped invocation). Used to
/// decide whether to show the friendly walkthrough and keep the window open.
pub fn interactive_session() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Keep a double-clicked console window open until the user has read the output.
pub fn pause() {
    print!("\nPress Enter to close this window...");
    let _ = io::stdout().flush();
    let mut discard = String::new();
    let _ = io::stdin().read_line(&mut discard);
}

/// Plain-language summary shown before an interactive install begins, with a
/// chance to back out by closing the window.
fn welcome_and_confirm() -> io::Result<()> {
    let launcher = if cfg!(windows) {
        "add a Start Menu shortcut"
    } else {
        "add an application launcher"
    };
    println!("Starsector System Ranker — Installer");
    println!();
    println!("This will set the program up just for your user account. It will:");
    println!("  - copy the program to a personal folder and add it to your PATH");
    println!("  - find your Starsector save and build its database");
    println!("  - {launcher} so you can open it easily");
    println!();
    println!("No administrator rights are needed, and your game files are never changed.");
    println!("You can undo all of this later by running: system_solver uninstall");
    println!();
    prompt("Press Enter to begin, or close this window to cancel... ")?;
    println!();
    Ok(())
}

/// Install the binary from the surrounding unpacked release archive.
pub fn run_install(opts: InstallOpts) -> Fallible {
    if !opts.yes && interactive_session() {
        welcome_and_confirm()?;
    }

    let source = std::env::current_exe()?;
    let archive_dir = source
        .parent()
        .ok_or("could not determine the archive directory")?
        .to_path_buf();
    let file_name = source
        .file_name()
        .ok_or("could not determine the executable name")?;

    let install_dir = binary_install_dir()?;
    fs::create_dir_all(&install_dir)?;
    let installed_binary = install_dir.join(file_name);
    copy_if_different(&source, &installed_binary)?;
    make_executable(&installed_binary)?;
    println!("Installed {}", installed_binary.display());

    ensure_path(&install_dir)?;

    if opts.skip_extract {
        println!("Skipped initial extraction.");
    } else {
        let starsector_dir = choose_starsector_dir(opts.starsector_dir.clone(), opts.yes)?;
        // Reuse the same in-process init the `init` subcommand runs: it saves
        // the config and builds the first extraction DB in the per-user dir.
        crate::run_init(Some(starsector_dir), None, true, None)?;
    }

    if opts.no_shortcut {
        println!("Skipped launcher creation.");
    } else {
        create_launcher(&installed_binary);
    }

    let home = home_dir()?;
    install_agent_skills(
        &archive_dir,
        opts.with_skills,
        opts.no_skills,
        opts.yes,
        &home,
    )?;

    println!();
    println!("Installation complete!");
    if opts.no_shortcut {
        println!("Open a new terminal, then run: system_solver tui");
    } else if cfg!(windows) {
        println!("Open the \"Starsector System Ranker\" shortcut from the Start Menu to begin,");
        println!("or open a new terminal and run: system_solver tui");
    } else {
        println!("Launch \"Starsector System Ranker\" from your applications menu to begin,");
        println!("or open a new terminal and run: system_solver tui");
    }
    Ok(())
}

/// Remove everything [`run_install`] created, leaving user data untouched.
pub fn run_uninstall() -> Fallible {
    let install_dir = binary_install_dir()?;
    let home = home_dir()?;

    let binary = install_dir.join(exe_file_name());
    remove_installed_binary(&binary)?;

    remove_path_entry(&install_dir)?;
    remove_launcher()?;
    remove_agent_skills(&home)?;

    // Clean up the install dir if we left it empty (Windows `Programs/<app>`).
    if install_dir.exists() && fs::read_dir(&install_dir)?.next().is_none() {
        let _ = fs::remove_dir(&install_dir);
    }

    println!("User data and settings were left untouched:");
    for path in user_data_paths() {
        println!("  {}", path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Starsector dir selection + skills (cross-platform)
// ---------------------------------------------------------------------------

fn choose_starsector_dir(
    provided: Option<PathBuf>,
    assume_yes: bool,
) -> Result<PathBuf, Box<dyn Error>> {
    if assume_yes {
        return match provided {
            Some(dir) => {
                println!("Using Starsector install: {}", dir.display());
                Ok(dir)
            }
            None => locate::detect_starsector_dir().ok_or_else(|| {
                "could not auto-detect Starsector; rerun with --starsector-dir <path>".into()
            }),
        };
    }

    let detected = provided.or_else(locate::detect_starsector_dir);
    if let Some(dir) = &detected {
        let answer = prompt(&format!(
            "Use Starsector install at {}? [Y/n] ",
            dir.display()
        ))?;
        if matches!(answer.to_lowercase().as_str(), "" | "y" | "yes") {
            return Ok(dir.clone());
        }
    }

    loop {
        let text = prompt("Enter the Starsector install directory: ")?;
        let text = text.trim().trim_matches('"').trim();
        if !text.is_empty() {
            return Ok(PathBuf::from(text));
        }
    }
}

fn install_agent_skills(
    archive_dir: &Path,
    force: bool,
    skip: bool,
    assume_yes: bool,
    home: &Path,
) -> io::Result<()> {
    if skip {
        println!("Skipped agent skill installation.");
        return Ok(());
    }
    // Non-interactive installs don't touch agent dirs unless explicitly asked.
    if assume_yes && !force {
        return Ok(());
    }

    let source = archive_dir.join("skills").join(SKILL_NAME);
    if !source.is_dir() {
        if force || !assume_yes {
            println!(
                "warning: bundled agent skill not found at {}",
                source.display()
            );
        }
        return Ok(());
    }

    let targets: Vec<(&str, PathBuf)> = agent_skill_targets(home)
        .into_iter()
        .filter(|(_, parent, _)| parent.is_dir())
        .map(|(name, _, target)| (name, target))
        .collect();
    if targets.is_empty() {
        if force {
            println!("No Claude Code or Codex agent directories found; skipped agent skill installation.");
        }
        return Ok(());
    }

    for (name, target) in targets {
        let should_install = if force {
            true
        } else {
            let answer = prompt(&format!(
                "Install the {SKILL_NAME} agent skill for {name}? [y/N] "
            ))?;
            matches!(answer.to_lowercase().as_str(), "y" | "yes")
        };
        if should_install {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_dir_all(&source, &target)?;
            println!("Installed {name} skill: {}", target.display());
        }
    }
    Ok(())
}

fn remove_agent_skills(home: &Path) -> io::Result<()> {
    for (name, _, target) in agent_skill_targets(home) {
        if target.is_dir() {
            fs::remove_dir_all(&target)?;
            println!("Removed {name} skill: {}", target.display());
        } else if target.exists() {
            fs::remove_file(&target)?;
            println!("Removed {name} skill file: {}", target.display());
        }
    }
    Ok(())
}

/// `(display name, agent home dir, skill target dir)` for each supported agent.
fn agent_skill_targets(home: &Path) -> Vec<(&'static str, PathBuf, PathBuf)> {
    vec![
        (
            "Claude Code",
            home.join(".claude"),
            home.join(".claude").join("skills").join(SKILL_NAME),
        ),
        (
            "Codex",
            home.join(".codex"),
            home.join(".codex").join("skills").join(SKILL_NAME),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Binary placement (cross-platform)
// ---------------------------------------------------------------------------

fn copy_if_different(source: &Path, target: &Path) -> io::Result<()> {
    // Avoid copying the file onto itself (e.g. re-running from the install dir).
    let same = fs::canonicalize(source)
        .ok()
        .zip(fs::canonicalize(target).ok())
        .is_some_and(|(a, b)| a == b);
    if same {
        return Ok(());
    }
    fs::copy(source, target)?;
    Ok(())
}

fn remove_installed_binary(binary: &Path) -> Fallible {
    if !binary.exists() {
        println!("No installed binary found at {}", binary.display());
        return Ok(());
    }
    match fs::remove_file(binary) {
        Ok(()) => {
            println!("Removed {}", binary.display());
            Ok(())
        }
        Err(err) => {
            // On Windows the running executable can't delete itself; schedule it.
            #[cfg(windows)]
            {
                schedule_delete(binary)?;
                println!(
                    "Scheduled removal of {} (it is currently running).",
                    binary.display()
                );
                Ok(())
            }
            #[cfg(not(windows))]
            {
                Err(Box::new(err))
            }
        }
    }
}

fn exe_file_name() -> &'static str {
    if cfg!(windows) {
        "system_solver.exe"
    } else {
        "system_solver"
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PATH string math (pure, unit-tested; only used on Windows)
// ---------------------------------------------------------------------------

fn split_paths(value: &str) -> Vec<&str> {
    value.split(';').filter(|part| !part.is_empty()).collect()
}

fn contains_path(parts: &[&str], dir: &str) -> bool {
    parts.iter().any(|part| part.eq_ignore_ascii_case(dir))
}

/// New PATH value with `dir` appended, or `None` if it is already present.
fn path_with_dir_added(current: &str, dir: &str) -> Option<String> {
    let parts = split_paths(current);
    if contains_path(&parts, dir) {
        return None;
    }
    let mut updated = parts;
    updated.push(dir);
    Some(updated.join(";"))
}

/// New PATH value with `dir` removed, or `None` if it was not present.
fn path_with_dir_removed(current: &str, dir: &str) -> Option<String> {
    let parts = split_paths(current);
    if !contains_path(&parts, dir) {
        return None;
    }
    let kept: Vec<&str> = parts
        .into_iter()
        .filter(|part| !part.eq_ignore_ascii_case(dir))
        .collect();
    Some(kept.join(";"))
}

// ---------------------------------------------------------------------------
// Launcher contents (pure, unit-tested; only used on Linux)
// ---------------------------------------------------------------------------

fn desktop_entry_contents(installed_binary: &Path) -> String {
    // Use the absolute installed path: the desktop environment may not have
    // ~/.local/bin on PATH even when an interactive shell does.
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Starsector System Ranker\n\
         Comment=Open the Starsector System Ranker TUI\n\
         Exec={} tui\n\
         Terminal=true\n\
         Categories=Game;Utility;\n",
        installed_binary.display()
    )
}

// ---------------------------------------------------------------------------
// Small console + home helpers
// ---------------------------------------------------------------------------

fn prompt(message: &str) -> io::Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn home_dir() -> Result<PathBuf, Box<dyn Error>> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| "could not determine the home directory".into())
}

/// Per-user data/config locations, printed by `uninstall` so users know what
/// was left behind. Uses the same resolution as the rest of the tool.
fn user_data_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(data) = crate::paths::data_dir() {
        paths.push(data);
    }
    if let Some(config) = crate::paths::config_dir() {
        if !paths.contains(&config) {
            paths.push(config);
        }
    }
    paths
}

// ===========================================================================
// Windows implementation
// ===========================================================================

#[cfg(windows)]
fn binary_install_dir() -> Result<PathBuf, Box<dyn Error>> {
    let root = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(root).join("Programs").join(APP_NAME))
}

#[cfg(windows)]
fn ensure_path(install_dir: &Path) -> Fallible {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let dir = install_dir.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
    let current: String = env.get_value("Path").unwrap_or_default();

    match path_with_dir_added(&current, &dir) {
        None => println!("{dir} is already on the user PATH."),
        Some(updated) => {
            env.set_raw_value(
                "Path",
                &RegValue {
                    bytes: encode_utf16(&updated),
                    vtype: REG_EXPAND_SZ,
                },
            )?;
            broadcast_environment_change();
            println!("Added {dir} to the user PATH. Open a new shell to use it.");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn remove_path_entry(install_dir: &Path) -> Fallible {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    let dir = install_dir.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
    let current: String = env.get_value("Path").unwrap_or_default();

    match path_with_dir_removed(&current, &dir) {
        None => println!("{dir} was not on the user PATH."),
        Some(updated) => {
            env.set_raw_value(
                "Path",
                &RegValue {
                    bytes: encode_utf16(&updated),
                    vtype: REG_EXPAND_SZ,
                },
            )?;
            broadcast_environment_change();
            println!("Removed {dir} from the user PATH. Open a new shell to see the change.");
        }
    }
    Ok(())
}

/// UTF-16LE bytes with a trailing NUL, as REG_EXPAND_SZ expects.
#[cfg(windows)]
fn encode_utf16(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Notify already-running shells/Explorer that the environment changed. Purely
/// cosmetic — new processes read the updated PATH regardless — so failures are
/// ignored.
#[cfg(windows)]
fn broadcast_environment_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let param: Vec<u16> = "Environment"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut result: usize = 0;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            param.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        );
    }
}

#[cfg(windows)]
fn create_launcher(installed_binary: &Path) {
    match windows_shortcut_path()
        .and_then(|shortcut| create_windows_shortcut(installed_binary, &shortcut))
    {
        Ok(shortcut) => println!("Created Start Menu shortcut: {}", shortcut.display()),
        Err(err) => println!("warning: could not create Start Menu shortcut: {err}"),
    }
}

#[cfg(windows)]
fn create_windows_shortcut(
    installed_binary: &Path,
    shortcut: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    use mslnk::ShellLink;

    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut link = ShellLink::new(installed_binary)?;
    link.set_arguments(Some("tui".to_string()));
    link.set_working_dir(Some(
        installed_binary
            .parent()
            .unwrap_or(installed_binary)
            .to_string_lossy()
            .into_owned(),
    ));
    link.create_lnk(shortcut)?;
    Ok(shortcut.to_path_buf())
}

#[cfg(windows)]
fn remove_launcher() -> Fallible {
    let shortcut = windows_shortcut_path()?;
    if shortcut.exists() {
        fs::remove_file(&shortcut)?;
        println!("Removed {}", shortcut.display());
    }
    Ok(())
}

#[cfg(windows)]
fn windows_shortcut_path() -> Result<PathBuf, Box<dyn Error>> {
    let appdata = std::env::var_os("APPDATA").ok_or("APPDATA is not set")?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(START_MENU_NAME))
}

/// Delete a path shortly after the current (running) process can release it.
/// Uses a detached `cmd` helper because Windows won't delete a running exe.
#[cfg(windows)]
fn schedule_delete(path: &Path) -> io::Result<()> {
    use std::process::{Command, Stdio};
    Command::new("cmd")
        .arg("/C")
        .arg(format!(
            "ping 127.0.0.1 -n 3 >nul & del /f /q \"{}\"",
            path.display()
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

// ===========================================================================
// Unix implementation
// ===========================================================================

#[cfg(not(windows))]
fn binary_install_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home_dir()?.join(".local").join("bin"))
}

#[cfg(not(windows))]
fn ensure_path(install_dir: &Path) -> Fallible {
    let dir = install_dir.to_string_lossy().to_string();
    let current = std::env::var("PATH").unwrap_or_default();
    if current.split(':').any(|part| part == dir) {
        println!("{dir} is already on PATH.");
    } else {
        println!("Add this to your shell profile if needed:");
        println!("  export PATH=\"{dir}:$PATH\"");
    }
    Ok(())
}

#[cfg(not(windows))]
fn remove_path_entry(_install_dir: &Path) -> Fallible {
    // Unix installs only print a profile hint; nothing persistent to undo.
    Ok(())
}

#[cfg(not(windows))]
fn create_launcher(installed_binary: &Path) {
    match create_desktop_entry(installed_binary) {
        Ok(entry) => println!("Created launcher: {}", entry.display()),
        Err(err) => println!("warning: could not create launcher: {err}"),
    }
}

#[cfg(not(windows))]
fn create_desktop_entry(installed_binary: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let entry = linux_desktop_entry_path()?;
    if let Some(parent) = entry.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&entry, desktop_entry_contents(installed_binary))?;
    make_executable(&entry)?;
    Ok(entry)
}

#[cfg(not(windows))]
fn remove_launcher() -> Fallible {
    let entry = linux_desktop_entry_path()?;
    if entry.exists() {
        fs::remove_file(&entry)?;
        println!("Removed {}", entry.display());
    }
    Ok(())
}

#[cfg(not(windows))]
fn linux_desktop_entry_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home_dir()?
        .join(".local")
        .join("share")
        .join("applications")
        .join(DESKTOP_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_add_appends_when_absent() {
        let updated = path_with_dir_added(r"C:\a;C:\b", r"C:\new").unwrap();
        assert_eq!(updated, r"C:\a;C:\b;C:\new");
    }

    #[test]
    fn path_add_is_noop_when_present_case_insensitively() {
        assert!(path_with_dir_added(r"C:\a;C:\New", r"c:\new").is_none());
    }

    #[test]
    fn path_add_handles_empty_current() {
        assert_eq!(path_with_dir_added("", r"C:\new").unwrap(), r"C:\new");
    }

    #[test]
    fn path_remove_drops_matching_entry() {
        let updated = path_with_dir_removed(r"C:\a;C:\new;C:\b", r"C:\New").unwrap();
        assert_eq!(updated, r"C:\a;C:\b");
    }

    #[test]
    fn path_remove_is_noop_when_absent() {
        assert!(path_with_dir_removed(r"C:\a;C:\b", r"C:\new").is_none());
    }

    #[test]
    fn skill_targets_live_under_agent_dirs() {
        let home = Path::new("/home/user");
        let targets = agent_skill_targets(home);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].1, home.join(".claude"));
        assert_eq!(
            targets[0].2,
            home.join(".claude").join("skills").join(SKILL_NAME)
        );
        assert_eq!(
            targets[1].2,
            home.join(".codex").join("skills").join(SKILL_NAME)
        );
    }

    #[test]
    fn desktop_entry_uses_absolute_binary_and_tui_arg() {
        let contents = desktop_entry_contents(Path::new("/home/user/.local/bin/system_solver"));
        assert!(contents.contains("Exec=/home/user/.local/bin/system_solver tui"));
        assert!(contents.contains("Name=Starsector System Ranker"));
        assert!(contents.contains("Terminal=true"));
    }

    #[test]
    fn copy_dir_all_copies_nested_files() {
        let base = std::env::temp_dir().join(format!("ssr_install_test_{}", std::process::id()));
        let src = base.join("src");
        let nested = src.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        fs::write(nested.join("b.txt"), b"b").unwrap();

        let dst = base.join("dst");
        copy_dir_all(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(dst.join("nested").join("b.txt")).unwrap(), b"b");

        let _ = fs::remove_dir_all(&base);
    }
}
