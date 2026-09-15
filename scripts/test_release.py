"""Exercise release guards locally without GitHub access or crate uploads."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseTagTests(unittest.TestCase):
    def test_release_jobs_read_the_repository_toolchain(self):
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        self.assertRegex(toolchain["channel"], r"^\d+\.\d+\.\d+$")
        self.assertIn("clippy", toolchain["components"])
        self.assertIn("rustfmt", toolchain["components"])
        workflow = (ROOT / ".github/workflows/release.yaml").read_text()
        readers = workflow.split("      - name: Read Rust toolchain\n")[1:]
        self.assertEqual(len(readers), 2)
        self.assertEqual(workflow.count("toolchain: ${{ steps.rust.outputs.version }}"), 2)
        for reader in readers:
            script = textwrap.dedent(reader.split("        run: |\n", 1)[1].split("\n      - name:", 1)[0])
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                # A different pin proves CI reads the file rather than hardcoding it.
                (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.96.0"\n')
                output = root / "output"
                subprocess.run(
                    ["bash", "-euo", "pipefail", "-c", script],
                    cwd=root,
                    env={**os.environ, "GITHUB_OUTPUT": str(output)},
                    check=True,
                    capture_output=True,
                )
                self.assertEqual(output.read_text(), "version=1.96.0\n")

    def test_workflow_tag_validation(self):
        workflow = (ROOT / ".github/workflows/release.yaml").read_text()
        validation = workflow.split("      - name: Validate tag and Cargo version\n", 1)[1]
        script = textwrap.dedent(validation.split("        run: |\n", 1)[1].split("\n      - name:", 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "Cargo.toml").write_text('[package]\nversion = "0.1.0"\n')
            for tag, valid in [
                ("v0.1.0", True),
                ("v0.1.1", False),
                ("0.1.0", False),
                ("v0.1.0-rc.1", False),
                ("v00.1.0", False),
                ("v0.1.0\n", False),
                ("v0.1.0; exit 0", False),
            ]:
                with self.subTest(tag=tag):
                    result = subprocess.run(
                        ["bash", "-euo", "pipefail", "-c", script],
                        cwd=directory,
                        env={**os.environ, "RELEASE_TAG": tag},
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode == 0, valid, result.stdout + result.stderr)


@unittest.skipUnless(shutil.which("just"), "just is required for release recipe tests")
class ReleaseRecipeTests(unittest.TestCase):
    def run_just(self, *args, cwd=ROOT):
        return subprocess.run(
            ["just", "--justfile", str(cwd / "Justfile"), *args],
            cwd=cwd,
            capture_output=True,
            text=True,
        )

    def test_version_bumps(self):
        for latest, bump, expected in [
            ("v0.1.0", "patch", "0.1.1"),
            ("v0.1.9", "minor", "0.2.0"),
            ("v0.9.9", "major", "1.0.0"),
            ("v1.9.9", "patch", "1.9.10"),
        ]:
            with self.subTest(latest=latest, bump=bump):
                result = self.run_just("release", "_next-version", latest, bump)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    def test_rejects_invalid_versions_and_bumps(self):
        for latest, bump in [
            ("v1.2", "patch"),
            ("v1.2.3.4", "patch"),
            ("v1.2.3-rc.1", "patch"),
            ("v01.2.3", "minor"),
            ("v1.2.3", "invalid"),
        ]:
            with self.subTest(latest=latest, bump=bump):
                self.assertNotEqual(self.run_just("release", "_next-version", latest, bump).returncode, 0)

    def test_rejects_feature_branch_and_dirty_main_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy(ROOT / "Justfile", root)
            shutil.copytree(ROOT / ".just", root / ".just")
            subprocess.run(["git", "init", "-q", "-b", "feature"], cwd=root, check=True)
            result = self.run_just("release", "_ensure-release-ready", cwd=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("release from main only", result.stderr)
            subprocess.run(["git", "symbolic-ref", "HEAD", "refs/heads/main"], cwd=root, check=True)
            result = self.run_just("release", "_ensure-release-ready", cwd=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("working tree and index must be clean", result.stderr)

    def test_aliases_parse_without_running_release(self):
        for bump in ["patch", "minor", "major"]:
            result = self.run_just("--dry-run", f"release-{bump}")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"_release {bump}", result.stderr)


if __name__ == "__main__":
    unittest.main()
