#!/usr/bin/env python3
"""Per-user installer for Starsector System Ranker release archives."""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import stat
import subprocess
import sys
from pathlib import Path
from typing import Iterable, Optional


APP_NAME = "StarsectorSystemRanker"
EXE_NAME = "system_solver.exe" if os.name == "nt" else "system_solver"
START_MENU_NAME = "Starsector System Ranker.lnk"
DESKTOP_FILE = "starsector-system-ranker.desktop"
SKILL_NAME = "system-solver"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Install Starsector System Ranker from an unpacked release archive."
    )
    parser.add_argument("--starsector-dir", type=Path, help="Starsector install directory")
    parser.add_argument("--no-shortcut", action="store_true", help="skip launcher creation")
    parser.add_argument("--skip-extract", action="store_true", help="skip initial DB extraction")
    skills = parser.add_mutually_exclusive_group()
    skills.add_argument(
        "--with-skills",
        action="store_true",
        help="install the bundled agent skill into existing Claude Code/Codex skill dirs",
    )
    skills.add_argument("--no-skills", action="store_true", help="skip agent skill installation")
    parser.add_argument("--yes", action="store_true", help="non-interactive install")
    parser.add_argument("--uninstall", action="store_true", help="remove installed files")
    args = parser.parse_args()

    try:
        if args.uninstall:
            uninstall()
        else:
            install(args)
    except InstallerError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("\nCanceled.", file=sys.stderr)
        return 1
    return 0


class InstallerError(RuntimeError):
    pass


def install(args: argparse.Namespace) -> None:
    archive_dir = Path(__file__).resolve().parent
    source_binary = archive_dir / EXE_NAME
    if not source_binary.is_file():
        raise InstallerError(
            f"missing {EXE_NAME}; run this installer from an unpacked release archive"
        )

    install_dir = binary_install_dir()
    install_dir.mkdir(parents=True, exist_ok=True)
    installed_binary = install_dir / EXE_NAME
    shutil.copy2(source_binary, installed_binary)
    make_executable(installed_binary)
    print(f"Installed {installed_binary}")

    ensure_path(install_dir)

    starsector_dir: Optional[Path] = args.starsector_dir
    if not args.skip_extract:
        starsector_dir = choose_starsector_dir(installed_binary, starsector_dir, args.yes)
        run_command(
            [
                str(installed_binary),
                "init",
                "--starsector-dir",
                str(starsector_dir),
                "--latest",
            ],
            "initial extraction",
        )
    else:
        print("Skipped initial extraction.")

    if args.no_shortcut:
        print("Skipped launcher creation.")
    elif os.name == "nt":
        create_windows_shortcut(installed_binary)
    else:
        create_desktop_entry(installed_binary)

    install_agent_skills(archive_dir, args.with_skills, args.no_skills, args.yes)

    print("Done. Open a new terminal, then run: system_solver tui")


def uninstall() -> None:
    install_dir = binary_install_dir()
    binary = install_dir / EXE_NAME
    if binary.exists():
        binary.unlink()
        print(f"Removed {binary}")
    else:
        print(f"No installed binary found at {binary}")

    remove_path_entry(install_dir)

    if os.name == "nt":
        shortcut = windows_shortcut_path()
    else:
        shortcut = linux_desktop_entry_path()
    if shortcut.exists():
        shortcut.unlink()
        print(f"Removed {shortcut}")

    remove_agent_skills()

    if install_dir.exists() and not any(install_dir.iterdir()):
        install_dir.rmdir()

    print("User data and settings were left untouched:")
    for path in user_data_paths():
        print(f"  {path}")


def binary_install_dir() -> Path:
    if os.name == "nt":
        root = os.environ.get("LOCALAPPDATA")
        if not root:
            raise InstallerError("LOCALAPPDATA is not set")
        return Path(root) / "Programs" / APP_NAME
    return Path.home() / ".local" / "bin"


def skill_source_dir(archive_dir: Path) -> Path:
    return archive_dir / "skills" / SKILL_NAME


def agent_skill_targets() -> list[tuple[str, Path, Path]]:
    home = Path.home()
    return [
        ("Claude Code", home / ".claude", home / ".claude" / "skills" / SKILL_NAME),
        ("Codex", home / ".codex", home / ".codex" / "skills" / SKILL_NAME),
    ]


def install_agent_skills(
    archive_dir: Path, force: bool, skip: bool, assume_yes: bool
) -> None:
    if skip:
        print("Skipped agent skill installation.")
        return
    if assume_yes and not force:
        return

    source = skill_source_dir(archive_dir)
    if not source.is_dir():
        if force or not assume_yes:
            print(f"warning: bundled agent skill not found at {source}")
        return

    targets = [(name, target) for name, parent, target in agent_skill_targets() if parent.is_dir()]
    if not targets:
        if force:
            print("No Claude Code or Codex agent directories found; skipped agent skill installation.")
        return

    for name, target in targets:
        should_install = force
        if not force:
            answer = input(f"Install the {SKILL_NAME} agent skill for {name}? [y/N] ")
            should_install = answer.strip().lower() in ("y", "yes")
        if should_install:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(source, target, dirs_exist_ok=True)
            print(f"Installed {name} skill: {target}")


def remove_agent_skills() -> None:
    for name, _, target in agent_skill_targets():
        if target.is_dir():
            shutil.rmtree(target)
            print(f"Removed {name} skill: {target}")
        elif target.exists():
            target.unlink()
            print(f"Removed {name} skill file: {target}")


def choose_starsector_dir(
    installed_binary: Path, provided: Optional[Path], assume_yes: bool
) -> Path:
    if assume_yes:
        if provided is None:
            detected = detect_starsector(installed_binary)
            if detected is None:
                raise InstallerError(
                    "could not auto-detect Starsector; rerun with --starsector-dir <path>"
                )
            return detected
        print(f"Using Starsector install: {provided}")
        return provided

    detected = provided or detect_starsector(installed_binary)
    if detected is not None:
        answer = input(f"Use Starsector install at {detected}? [Y/n] ").strip().lower()
        if answer in ("", "y", "yes"):
            return detected

    while True:
        text = input("Enter the Starsector install directory: ").strip().strip('"')
        if text:
            return Path(text).expanduser()


def detect_starsector(installed_binary: Path) -> Optional[Path]:
    print("Looking for Starsector...")
    result = subprocess.run(
        [str(installed_binary), "locate"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        message = result.stderr.strip()
        if message:
            print(message)
        return None

    print(result.stdout.rstrip())
    for line in result.stdout.splitlines():
        if line.startswith("Starsector:"):
            return Path(line.split(":", 1)[1].strip())
    return None


def ensure_path(path: Path) -> None:
    if os.name == "nt":
        add_windows_path(path)
        return

    path_text = str(path)
    current = os.environ.get("PATH", "")
    if path_text in current.split(os.pathsep):
        print(f"{path_text} is already on PATH.")
    else:
        print(f"Add this to your shell profile if needed:")
        print(f'  export PATH="{path_text}:$PATH"')


def add_windows_path(path: Path) -> None:
    path_text = str(path)
    current = user_windows_path()
    parts = split_path(current)
    if any(os.path.normcase(part) == os.path.normcase(path_text) for part in parts):
        print(f"{path_text} is already on the user PATH.")
        return

    updated = os.pathsep.join(parts + [path_text]) if parts else path_text
    run_reg(["add", r"HKCU\Environment", "/v", "Path", "/t", "REG_EXPAND_SZ", "/d", updated, "/f"])
    broadcast_environment_change()
    print(f"Added {path_text} to the user PATH. Open a new shell to use it.")


def remove_path_entry(path: Path) -> None:
    if os.name != "nt":
        return

    path_text = str(path)
    current = user_windows_path()
    parts = [
        part
        for part in split_path(current)
        if os.path.normcase(part) != os.path.normcase(path_text)
    ]
    if len(parts) == len(split_path(current)):
        print(f"{path_text} was not on the user PATH.")
        return

    run_reg(["add", r"HKCU\Environment", "/v", "Path", "/t", "REG_EXPAND_SZ", "/d", os.pathsep.join(parts), "/f"])
    broadcast_environment_change()
    print(f"Removed {path_text} from the user PATH. Open a new shell to see the change.")


def user_windows_path() -> str:
    result = subprocess.run(
        ["reg", "query", r"HKCU\Environment", "/v", "Path"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if result.returncode != 0:
        return ""
    for line in result.stdout.splitlines():
        if "Path" in line:
            fields = line.split(maxsplit=2)
            if len(fields) == 3:
                return fields[2]
    return ""


def split_path(value: str) -> list[str]:
    return [part for part in value.split(os.pathsep) if part]


def run_reg(args: Iterable[str]) -> None:
    result = subprocess.run(["reg", *args], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode != 0:
        raise InstallerError(result.stderr.strip() or "failed to update the user PATH")


def broadcast_environment_change() -> None:
    if os.name != "nt":
        return
    script = (
        "Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition "
        "'[DllImport(\"user32.dll\", SetLastError=true, CharSet=CharSet.Auto)] "
        "public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, "
        "UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'; "
        "$result=[UIntPtr]::Zero; "
        "[Win32.NativeMethods]::SendMessageTimeout([IntPtr]0xffff,0x1a,[UIntPtr]::Zero,"
        "'Environment',2,5000,[ref]$result) | Out-Null"
    )
    subprocess.run(["powershell", "-NoProfile", "-Command", script], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def create_windows_shortcut(installed_binary: Path) -> None:
    shortcut = windows_shortcut_path()
    shortcut.parent.mkdir(parents=True, exist_ok=True)
    script = (
        "$shell = New-Object -ComObject WScript.Shell; "
        f"$shortcut = $shell.CreateShortcut('{ps_quote(str(shortcut))}'); "
        f"$shortcut.TargetPath = '{ps_quote(str(installed_binary))}'; "
        "$shortcut.Arguments = 'tui'; "
        f"$shortcut.WorkingDirectory = '{ps_quote(str(installed_binary.parent))}'; "
        "$shortcut.Save()"
    )
    result = subprocess.run(
        ["powershell", "-NoProfile", "-Command", script],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode == 0:
        print(f"Created Start Menu shortcut: {shortcut}")
    else:
        print(f"warning: could not create Start Menu shortcut: {result.stderr.strip()}")


def create_desktop_entry(installed_binary: Path) -> None:
    entry = linux_desktop_entry_path()
    entry.parent.mkdir(parents=True, exist_ok=True)
    # Use the absolute installed path: the desktop environment may not have
    # ~/.local/bin on PATH even when an interactive shell does.
    entry.write_text(
        "\n".join(
            [
                "[Desktop Entry]",
                "Type=Application",
                "Name=Starsector System Ranker",
                "Comment=Open the Starsector System Ranker TUI",
                f"Exec={installed_binary} tui",
                "Terminal=true",
                "Categories=Game;Utility;",
                "",
            ]
        ),
        encoding="utf-8",
    )
    make_executable(entry)
    print(f"Created launcher: {entry}")


def windows_shortcut_path() -> Path:
    appdata = os.environ.get("APPDATA")
    if not appdata:
        raise InstallerError("APPDATA is not set")
    return Path(appdata) / "Microsoft" / "Windows" / "Start Menu" / "Programs" / START_MENU_NAME


def linux_desktop_entry_path() -> Path:
    return Path.home() / ".local" / "share" / "applications" / DESKTOP_FILE


def make_executable(path: Path) -> None:
    if os.name != "nt":
        path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def run_command(command: list[str], label: str) -> None:
    print(f"Running {label}...")
    result = subprocess.run(command)
    if result.returncode != 0:
        raise InstallerError(f"{label} failed with exit code {result.returncode}")


def ps_quote(value: str) -> str:
    return value.replace("'", "''")


def user_data_paths() -> list[Path]:
    if os.name == "nt":
        appdata = Path(os.environ.get("APPDATA", str(Path.home() / "AppData" / "Roaming")))
        local = Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData" / "Local")))
        return [appdata / APP_NAME, local / APP_NAME]
    if platform.system() == "Darwin":
        return [
            Path.home() / "Library" / "Application Support" / APP_NAME,
            Path.home() / "Library" / "Preferences" / APP_NAME,
        ]
    return [Path.home() / ".local" / "share" / APP_NAME, Path.home() / ".config" / APP_NAME]


if __name__ == "__main__":
    raise SystemExit(main())
