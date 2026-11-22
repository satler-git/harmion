#!/usr/bin/env python3
"""Static user-unit validation; never installs files or contacts a user manager."""
from pathlib import Path
import configparser
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
UNIT = ROOT / "deploy/systemd/harmion@.service"


def main():
    text = UNIT.read_text(encoding="utf-8")
    config = configparser.ConfigParser(interpolation=None, strict=True)
    config.optionxform = str
    config.read_string(text)
    expected = {
        "Unit": {
            "Description": "Harmion user node (%i)",
            "StartLimitIntervalSec": "60",
            "StartLimitBurst": "3",
        },
        "Service": {
            "Type": "simple",
            "ExecStart": '"%h/.local/bin/harmiond" run "%h/.local/share/harmion/%i" --wake-listen 127.0.0.1:8787',
            "UMask": "0077",
            "Restart": "on-failure",
            "RestartSec": "5s",
            "KillSignal": "SIGTERM",
            "TimeoutStopSec": "60s",
            "NoNewPrivileges": "yes",
            "StandardOutput": "journal",
            "StandardError": "journal",
            "SyslogIdentifier": "harmiond-%i",
        },
        "Install": {"WantedBy": "default.target"},
    }
    actual = {section: dict(config[section]) for section in config.sections()}
    if actual != expected:
        raise AssertionError(f"unexpected unit directives: {actual!r}")
    # Exact allowlist also excludes automatic initialization, credentials in env,
    # root-only User/DynamicUser, notify/watchdog, and untested namespace policy.
    print("PASS: user-unit paths, permissions, restart/stop policy, directive allowlist")

    analyzer = shutil.which("systemd-analyze")
    if analyzer is None:
        print("SKIP: systemd-analyze unavailable; syntax not verified by systemd")
        return
    with tempfile.TemporaryDirectory(prefix="harmion unit check ") as temporary:
        stage = Path(temporary)
        executable = stage / "harmiond"
        executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        executable.chmod(0o700)
        staged = stage / UNIT.name
        # A path with spaces exercises systemd's quoting. This does not run it.
        staged.write_text(
            text.replace(
                '"%h/.local/bin/harmiond"', f'"{executable}"', 1
            ),
            encoding="utf-8",
        )
        # Keep user-unit lookup usable on non-systemd distributions, including
        # Nix dev environments without a default.target installed in /usr/lib.
        # This private dependency stub tests syntax, not host target availability.
        for target in ("default.target", "basic.target", "shutdown.target"):
            (stage / target).write_text(
                "[Unit]\nDescription=Static validation target\nDefaultDependencies=no\n",
                encoding="utf-8",
            )
        environment = os.environ.copy()
        environment["SYSTEMD_UNIT_PATH"] = str(stage)
        command = [analyzer, "--user", "verify", str(staged)]
        result = subprocess.run(command, env=environment, text=True, capture_output=True)
        if result.returncode:
            raise AssertionError(
                f"systemd-analyze exited {result.returncode}\n"
                f"{result.stdout}{result.stderr}"
            )
        if result.stdout or result.stderr:
            print(result.stdout + result.stderr, end="")
        print("PASS: systemd-analyze --user verify (temporary executable/target; no service started)")


if __name__ == "__main__":
    main()
