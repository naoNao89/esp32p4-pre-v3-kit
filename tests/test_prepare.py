import sys
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

KIT_ROOT = Path(__file__).resolve().parents[1]
PINNED_PACKAGES = {
    "esp-hal": "1.1.0",
    "esp-backtrace": "0.19.0",
    "esp-bootloader-esp-idf": "0.5.0",
    "esp-println": "0.17.0",
}


class PrepareTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="esp-hal-kit ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.kit = self.root / "kit"
        self.kit.mkdir()
        (self.kit / "patches").mkdir()
        self.source = self.root / "source"
        self.source.mkdir()
        self._make_source_repository()
        self.patch = self._make_patch()
        self.patch_path = self.kit / "patches" / "fixture.patch"
        self.patch_path.write_bytes(self.patch)
        self._write_manifest()

    def _git(self, *args):
        result = subprocess.run(
            ["git", "-C", str(self.source), *args],
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()

    def _make_source_repository(self):
        subprocess.run(["git", "init", "--quiet", str(self.source)], check=True)
        self._git("config", "user.name", "Prepare Fixture")
        self._git("config", "user.email", "prepare-fixture@example.invalid")
        for package, version in PINNED_PACKAGES.items():
            package_dir = self.source / package
            (package_dir / "src").mkdir(parents=True)
            (package_dir / "Cargo.toml").write_text(
                "[package]\n"
                f'name = "{package}"\n'
                f'version = "{version}"\n'
                'edition = "2024"\n\n'
                '[lib]\npath = "src/lib.rs"\n',
                encoding="utf-8",
            )
            source = (
                'pub const SOURCE: &str = "pinned";\n'
                if package == "esp-hal"
                else "pub fn fixture() {}\n"
            )
            (package_dir / "src" / "lib.rs").write_text(source, encoding="utf-8")
        self._git("add", ".")
        self._git("commit", "--quiet", "-m", "pinned source")
        self.revision = self._git("rev-parse", "HEAD")

    def _make_patch(self):
        source_file = self.source / "esp-hal" / "src" / "lib.rs"
        source_file.write_text(
            'pub const SOURCE: &str = "patched";\n', encoding="utf-8"
        )
        (self.source / "esp-hal" / "src" / "pre-v3.rom").write_bytes(
            b"\x00pre-v3\xff"
        )
        self._git("add", ".")
        self.expected_tree = self._git("write-tree")
        patch = subprocess.run(
            ["git", "-C", str(self.source), "diff", "--cached", "--binary"],
            check=True,
            capture_output=True,
        ).stdout
        self._git("reset", "--hard", self.revision)
        (self.source / "tip-only.txt").write_text("must not be installed\n", encoding="utf-8")
        self._git("add", "tip-only.txt")
        self._git("commit", "--quiet", "-m", "moving tip")
        return patch

    def _write_manifest(self, repository=None, digest=None, patch_path=None):
        manifest = {
            "upstream": {
                "repository": str(repository or self.source),
                "revision": self.revision,
            },
            "patch": {
                "path": patch_path or "patches/fixture.patch",
                "sha256": digest or hashlib.sha256(self.patch).hexdigest(),
            },
            "cargo_patches": list(PINNED_PACKAGES),
        }
        (self.kit / "patches" / "manifest.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )

    def _prepare(self, destination):
        source_script = KIT_ROOT / "prepare.py"
        if source_script.is_file():
            shutil.copy2(source_script, self.kit / "prepare.py")
        return subprocess.run(
            [sys.executable, str(self.kit / "prepare.py"), str(destination)],
            cwd=self.kit,
            capture_output=True,
            text=True,
        )

    def test_prepare_keeps_relative_destination_in_invoking_directory(self):
        for filename in ("Cargo.toml", "xtask.rs", "prepare.py"):
            shutil.copy2(KIT_ROOT / filename, self.kit / filename)
        (self.kit / ".cargo").mkdir()
        shutil.copy2(
            KIT_ROOT / ".cargo" / "config.toml",
            self.kit / ".cargo" / "config.toml",
        )

        nested = self.kit / "nested"
        nested.mkdir()
        env = os.environ.copy()
        env["CARGO_NET_OFFLINE"] = "true"
        env["CARGO_TARGET_DIR"] = str(self.root / "cargo-target")
        result = subprocess.run(
            ["cargo", "xtask", "prepare", "prepared"],
            cwd=nested,
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
        )
        self.assertEqual(result.returncode, 0, msg=f"{result.stdout}\n{result.stderr}")
        self.assertTrue((nested / "prepared" / "upstream").is_dir())
        self.assertFalse((self.kit / "prepared").exists())

    def test_installs_the_pinned_patched_tree_and_emits_consumable_cargo_paths(self):
        destination = self.root / "prepared"
        result = self._prepare(destination)
        self.assertEqual(result.returncode, 0, msg=result.stderr)

        installed_source = destination / "upstream"
        for expression, expected in (
            ("HEAD", self.revision),
            (None, self.expected_tree),
        ):
            if expression is None:
                subprocess.run(
                    ["git", "-C", str(installed_source), "add", "."], check=True
                )
            actual = subprocess.check_output(
                ["git", "-C", str(installed_source), "rev-parse", expression]
                if expression
                else ["git", "-C", str(installed_source), "write-tree"],
                text=True,
            ).strip()
            self.assertEqual(actual, expected)

        consumer = self.root / "consumer"
        consumer.mkdir()
        (consumer / "src").mkdir()
        (consumer / "src" / "lib.rs").write_text(
            "pub use esp_hal::SOURCE;\n", encoding="utf-8"
        )
        manifest = consumer / "Cargo.toml"
        manifest.write_text(
            '[package]\nname = "fixture-consumer"\nversion = "0.1.0"\nedition = "2024"\n\n'
            "[dependencies]\n"
            + "".join(
                f'{package} = "={version}"\n'
                for package, version in PINNED_PACKAGES.items()
            )
            + "\n"
            + (destination / "Cargo.patch.toml").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        metadata = subprocess.run(
            [
                "cargo",
                "metadata",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ],
            cwd=consumer,
            capture_output=True,
            text=True,
        )
        self.assertEqual(metadata.returncode, 0, msg=metadata.stderr)
        packages = json.loads(metadata.stdout)["packages"]
        installed = {
            package["name"]: Path(package["manifest_path"])
            for package in packages
            if package["name"] in PINNED_PACKAGES
        }
        self.assertEqual(set(installed), set(PINNED_PACKAGES))
        for package in PINNED_PACKAGES:
            self.assertEqual(
                installed[package],
                destination / "upstream" / package / "Cargo.toml",
            )

    def test_rejects_bad_patch_digest_before_fetching_or_creating_destination(self):
        self._write_manifest(
            repository=self.root / "missing-source",
            digest="0" * 64,
        )
        destination = self.root / "prepared"

        result = self._prepare(destination)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sha256", result.stderr.lower())
        self.assertFalse(destination.exists())

    def test_preserves_existing_directory_and_dangling_symlink_destinations(self):
        existing = self.root / "existing"
        existing.mkdir()
        marker = existing / "keep.txt"
        marker.write_text("user data\n", encoding="utf-8")

        result = self._prepare(existing)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("destination", result.stderr.lower())
        self.assertEqual(marker.read_text(encoding="utf-8"), "user data\n")
        self.assertFalse((existing / "upstream").exists())

        link = self.root / "dangling-link"
        link.symlink_to(self.root / "not-created")
        link_result = self._prepare(link)

        self.assertNotEqual(link_result.returncode, 0)
        self.assertIn("destination", link_result.stderr.lower())
        self.assertTrue(link.is_symlink())
        self.assertFalse((self.root / "not-created").exists())

    def test_failed_patch_application_leaves_no_partial_destination(self):
        invalid_patch = (
            b"diff --git a/esp-hal/src/not-present.txt b/esp-hal/src/not-present.txt\n"
            b"--- a/esp-hal/src/not-present.txt\n"
            b"+++ b/esp-hal/src/not-present.txt\n"
            b"@@ -1 +1 @@\n-old\n+new\n"
        )
        self.patch_path.write_bytes(invalid_patch)
        self._write_manifest(digest=hashlib.sha256(invalid_patch).hexdigest())
        destination = self.root / "prepared"

        result = self._prepare(destination)

        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(destination.exists())

class XTaskPrepareTests(PrepareTests):
    def setUp(self):
        super().setUp()
        self.xtask_target = self.root / "xtask_target"
        subprocess.run(
            ["cargo", "build", "--bin", "xtask"],
            cwd=KIT_ROOT,
            env={**os.environ, "CARGO_TARGET_DIR": str(self.xtask_target)},
            check=True,
            capture_output=True,
        )
        self.xtask_bin = self.xtask_target / "debug" / "xtask"

    def test_xtask_prepare_with_custom_python_executable(self):
        if sys.platform == "win32":
            self.skipTest("Unix-specific wrapper test")

        wrapper = self.root / "custom_python.sh"
        wrapper.write_text(f"#!/bin/sh\nexec {sys.executable} \"$@\"\n")
        wrapper.chmod(0o755)

        # Clear PATH of python3 to ensure xtask MUST use PYTHON
        env = os.environ.copy()
        env["PYTHON"] = str(wrapper)
        env["CARGO_MANIFEST_DIR"] = str(self.kit)
        
        # Create a new isolated bin directory with only git, to prove python3 is absent
        isolated_bin = self.root / "isolated_bin"
        isolated_bin.mkdir()
        git_path = shutil.which("git")
        if git_path:
            (isolated_bin / "git").symlink_to(git_path)
        env["PATH"] = str(isolated_bin)
        
        for filename in ("Cargo.toml", "xtask.rs", "prepare.py"):
            shutil.copy2(KIT_ROOT / filename, self.kit / filename)
            
        destination = self.root / "prepared"
        
        result = subprocess.run(
            [str(self.xtask_bin), "prepare", str(destination)],
            env=env,
            cwd=self.kit,
            capture_output=True,
            text=True
        )
        self.assertEqual(result.returncode, 0, f"xtask prepare failed: {result.stderr}")
        self.assertTrue(destination.is_dir(), "Destination directory was not created")
        self.assertTrue((destination / "upstream" / "esp-hal" / "Cargo.toml").exists(), "esp-hal package not found in destination")
    def test_xtask_prepare_yields_real_nonzero_error_on_failure(self):
        # Test that xtask propagates failure
        destination = self.root / "prepared"
        
        # Create the destination ahead of time so prepare.py fails
        destination.mkdir()
        
        env = os.environ.copy()
        
        result = subprocess.run(
            [str(self.xtask_bin), "prepare", str(destination)],
            env=env,
            cwd=self.kit,
            capture_output=True,
            text=True
        )
        
        self.assertNotEqual(result.returncode, 0, f"xtask prepare should have failed due to existing destination, but succeeded. STDOUT: {result.stdout}")
        self.assertIn("destination already exists", result.stderr, "Did not get expected prepare.py error")
if __name__ == "__main__":
    unittest.main()
