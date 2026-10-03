#!/usr/bin/env python3
"""Prepare the pinned, patched upstream HAL checkout for a consumer workspace."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


class PrepareError(Exception):
    pass


def _run(step, command):
    try:
        result = subprocess.run(command, capture_output=True, text=True)
    except OSError as error:
        raise PrepareError(f"{step} failed to start: {error}") from error
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        suffix = f": {detail}" if detail else ""
        raise PrepareError(f"{step} failed (exit {result.returncode}){suffix}")
    return result.stdout.strip()


def prepare(destination_argument):
    destination = Path(os.path.abspath(destination_argument))
    if os.path.lexists(destination):
        raise PrepareError(f"destination already exists: {destination}")
    if not destination.parent.is_dir():
        raise PrepareError(f"destination parent does not exist or is not a directory: {destination.parent}")

    kit_root = Path(__file__).resolve().parent
    manifest_path = kit_root / "patches" / "manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        repository = manifest["upstream"]["repository"]
        revision = manifest["upstream"]["revision"]
        patch_path = kit_root / manifest["patch"]["path"]
        expected_sha256 = manifest["patch"]["sha256"]
        cargo_patches = manifest["cargo_patches"]
        patch_bytes = patch_path.read_bytes()
    except (OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError) as error:
        raise PrepareError(f"read patch plan or patch failed: {error}") from error

    actual_sha256 = hashlib.sha256(patch_bytes).hexdigest()
    if actual_sha256 != expected_sha256:
        raise PrepareError(
            f"patch SHA256 mismatch: expected {expected_sha256}, got {actual_sha256}"
        )
    if not isinstance(repository, str) or not isinstance(revision, str):
        raise PrepareError("read patch plan failed: upstream repository and revision must be strings")
    if not isinstance(cargo_patches, list) or any(
        not isinstance(package, str)
        or not package
        or Path(package).name != package
        or package in (".", "..")
        for package in cargo_patches
    ):
        raise PrepareError("read patch plan failed: cargo_patches must be package directory names")

    try:
        with tempfile.TemporaryDirectory(
            prefix=f".{destination.name}.prepare-", dir=str(destination.parent)
        ) as temporary_directory:
            staging = Path(temporary_directory)
            staged_patch = staging / "verified.patch"
            try:
                staged_patch.write_bytes(patch_bytes)
            except OSError as error:
                raise PrepareError(f"stage verified patch failed: {error}") from error

            upstream = staging / "upstream"
            _run("initialize temporary checkout", ["git", "init", "--quiet", str(upstream)])
            _run(
                "configure upstream remote",
                ["git", "-C", str(upstream), "remote", "add", "origin", repository],
            )
            _run(
                "fetch pinned revision",
                [
                    "git",
                    "-C",
                    str(upstream),
                    "fetch",
                    "--no-tags",
                    "--depth=1",
                    "origin",
                    revision,
                ],
            )
            _run(
                "checkout pinned revision",
                ["git", "-C", str(upstream), "checkout", "--quiet", "--detach", "FETCH_HEAD"],
            )
            head = _run("verify pinned revision", ["git", "-C", str(upstream), "rev-parse", "HEAD"])
            if head != revision:
                raise PrepareError(f"verify pinned revision failed: expected {revision}, got {head}")

            _run(
                "check patch application",
                ["git", "-C", str(upstream), "apply", "--check", str(staged_patch)],
            )
            _run(
                "apply source patch",
                ["git", "-C", str(upstream), "apply", str(staged_patch)],
            )

            for package in cargo_patches:
                if not (upstream / package).is_dir():
                    raise PrepareError(
                        f"verify patched checkout failed: missing package directory {package}"
                    )

            candidate = staging / "prepared"
            try:
                candidate.mkdir()
                os.rename(upstream, candidate / "upstream")
            except OSError as error:
                raise PrepareError(f"stage prepared checkout failed: {error}") from error

            patch_lines = ["[patch.crates-io]"]
            for package in cargo_patches:
                package_path = destination / "upstream" / package
                patch_lines.append(
                    f"{json.dumps(package)} = {{ path = {json.dumps(str(package_path), ensure_ascii=False)} }}"
                )
            try:
                (candidate / "Cargo.patch.toml").write_text(
                    "\n".join(patch_lines) + "\n", encoding="utf-8"
                )
            except OSError as error:
                raise PrepareError(f"write Cargo.patch.toml failed: {error}") from error

            try:
                os.mkdir(destination)
            except FileExistsError as error:
                raise PrepareError(f"destination already exists: {destination}") from error
            except OSError as error:
                raise PrepareError(f"create destination failed: {error}") from error

            reserved = os.lstat(destination)
            try:
                os.rename(candidate, destination)
            except OSError as error:
                try:
                    current = os.lstat(destination)
                    if (current.st_dev, current.st_ino) == (reserved.st_dev, reserved.st_ino):
                        os.rmdir(destination)
                except OSError:
                    pass
                raise PrepareError(f"publish prepared checkout failed: {error}") from error
    except PrepareError:
        raise
    except OSError as error:
        raise PrepareError(f"create staging area failed: {error}") from error

    print(f"Prepared patched upstream source at {destination / 'upstream'}")
    print(
        "To use it, add the contents of "
        f"{destination / 'Cargo.patch.toml'} to the consumer workspace-root Cargo.toml."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", help="new directory for the prepared source")
    arguments = parser.parse_args()
    try:
        prepare(arguments.destination)
    except PrepareError as error:
        print(f"prepare: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
