#!/usr/bin/env python3
"""Validate and pack one native Silicon Apps archive without signing in or publishing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {"linux-x86_64", "linux-aarch64", "windows-x86_64", "windows-aarch64", "macos-x86_64", "macos-aarch64"}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("target", choices=sorted(TARGETS))
    parser.add_argument("binary", type=Path)
    parser.add_argument("--apps", default=os.environ.get("SILICON_APPS_PACKER", "silicon-apps"))
    args = parser.parse_args()
    expected = tomllib.loads((ROOT / "clients/rust/Cargo.toml").read_text())["workspace"]["package"]["version"]
    if args.version != expected:
        parser.error(f"version must match the CLI workspace ({expected})")
    executable = args.binary.resolve(strict=True)
    # Check native binary identity and the Linux glibc ceiling before shipping.
    from package_release_binary import verify_binary
    verify_binary(executable, args.target)
    native = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    machine = "aarch64" if platform.machine().lower() in {"arm64", "aarch64"} else "x86_64"
    if args.target != f"{native}-{machine}":
        parser.error("Run packaging on its native target runner so discovery commands can be verified")
    archive_dir = ROOT / "dist/apps"
    archive_dir.mkdir(parents=True, exist_ok=True)
    archive = archive_dir / f"briefcase-{args.version}-{args.target}.tar.gz"
    with tempfile.TemporaryDirectory(prefix="briefcase-apps-") as temporary:
        directory = Path(temporary)
        stage = directory / "package"
        (stage / "bin").mkdir(parents=True)
        binary_name = "briefcase.exe" if args.target.startswith("windows-") else "briefcase"
        destination = stage / "bin" / binary_name
        shutil.copy2(executable, destination)
        destination.chmod(0o755)
        manifest = (ROOT / "packaging/apps.yaml.in").read_text().replace("@VERSION@", args.version).replace("@TARGET@", args.target).replace("@BINARY@", binary_name)
        (stage / "apps.yaml").write_text(manifest)
        # Isolated homes with no inherited deployment or access credentials.
        env = {key: value for key, value in os.environ.items() if key in {"PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"}}
        home = directory / "home"
        home.mkdir()
        env.update(HOME=str(home), USERPROFILE=str(home), SILICON_HOME=str(home), SILICON_APPS_NO_DAEMON="1")
        subprocess.run([str(destination), "--help"], env=env, stdout=subprocess.DEVNULL, check=True, timeout=15)
        for command, key, expected_value in [( ["accounts", "--json"], "app_id", "briefcase" ), (["login", "status", "--json"], "authenticated", False)]:
            output = subprocess.run([str(destination), *command], env=env, capture_output=True, text=True, check=True, timeout=15)
            if json.loads(output.stdout).get(key) != expected_value:
                raise SystemExit(f"Discovery command {command} did not report {key}={expected_value!r}")
        apps_home = directory / "apps-home"
        apps_home.mkdir()
        packer = [args.apps, "--home", str(apps_home), "--server", "http://127.0.0.1:9"]
        subprocess.run([*packer, "validate", str(stage)], env=env, check=True)
        subprocess.run([*packer, "pack", str(stage), "--output", str(archive)], env=env, check=True)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    print(archive)

if __name__ == "__main__":
    main()
