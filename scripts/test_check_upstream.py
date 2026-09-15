import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import check_upstream as monitor


class MonitorTests(unittest.TestCase):
    def test_latest_stable_uses_numeric_order_and_ignores_prereleases(self):
        refs = "\n".join(f"abc refs/tags/{tag}" for tag in [
            "v0.9.99", "v0.10.1", "v0.10.2", "v0.11.0-alpha.1",
            "v0.11.0-rc.1", "v0.10.2^{}", "kubernetes-1.37.0",
        ])
        self.assertEqual(monitor.latest_stable(refs), "v0.10.2")
        with self.assertRaises(ValueError):
            monitor.latest_stable("abc refs/tags/v0.38.0-alpha.1")

    def test_rejects_invalid_versions(self):
        for value in ["main", "v1.37.0", "v0.37.0-rc.1", "--upload-pack=bad", "v0.37.0; echo bad"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                monitor.version_key(value)

    def test_config_validation(self):
        good = {"baseline": "v0.37.0", "sources": [{"repository": "kubernetes/kubectl", "paths": ["pkg/describe"]}]}
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "config.json"
            path.write_text(json.dumps(good))
            self.assertEqual(monitor.load_config(path), good)
            for key, bad in [("paths", ["../secret"]), ("paths", ["/tmp/secret"]),
                             ("paths", []), ("paths", ["."]),
                             ("repository", "other/kubectl"), ("repository", "kubernetes/../secret")]:
                invalid = json.loads(json.dumps(good))
                invalid["sources"][0][key] = bad
                path.write_text(json.dumps(invalid))
                with self.subTest(key=key, value=bad), self.assertRaises(ValueError):
                    monitor.load_config(path)

    def test_checked_in_config_is_valid(self):
        monitor.load_config(monitor.ROOT / "upstream.json")

    def test_fetch_failure_is_not_reported_as_unchanged(self):
        config = {"baseline": "v0.37.0", "sources": [{"repository": "kubernetes/kubectl", "paths": ["pkg/describe"]}]}
        with tempfile.TemporaryDirectory() as temp, patch.object(monitor, "git", side_effect=RuntimeError("network failed")):
            report = monitor.check(config, "", Path(temp))
            self.assertEqual(report["status"], "error")
            self.assertIn("network failed", report["error"])
            self.assertEqual(monitor.exit_code(report, True), 1)

    def test_partial_failures_keep_successful_reports(self):
        config = {"baseline": "v0.37.0", "sources": [{"repository": "kubernetes/kubectl"}, {"repository": "kubernetes/apimachinery"}]}
        with tempfile.TemporaryDirectory() as temp, patch.object(monitor, "compare_source", side_effect=[
            {"repository": "kubernetes/kubectl", "status": "changed"}, RuntimeError("missing tag"),
        ]):
            report = monitor.check(config, "v0.37.1", Path(temp))
        self.assertEqual(report["status"], "error")
        self.assertEqual(report["sources"][0]["status"], "changed")
        self.assertIn("missing tag", report["sources"][1]["error"])

    def test_review_required_has_a_distinct_exit_code(self):
        self.assertEqual(monitor.exit_code({"status": "changed"}, True), 3)
        self.assertEqual(monitor.exit_code({"status": "changed"}, False), 0)
        self.assertEqual(monitor.exit_code({"status": "unchanged"}, True), 0)


class LocalGitTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "upstream"
        self.repo.mkdir()
        self.output = self.root / "reports"
        self.output.mkdir()
        self.source = {"repository": "kubernetes/kubectl", "paths": ["pkg/describe"]}
        self.git("init", "--quiet")
        directory = self.repo / "pkg/describe"
        directory.mkdir(parents=True)
        (directory / "describe.go").write_text("old implementation\n")
        (directory / "removed.go").write_text("old helper\n")
        self.commit("v0.37.0")
        (directory / "describe.go").write_text("new implementation\n")
        (directory / "removed.go").unlink()
        (directory / "added.go").write_text("new helper\n")
        (self.repo / "untracked-scope.go").write_text("outside tracked scope\n")
        self.commit("v0.37.1")

    def git(self, *args):
        return subprocess.run([
            "git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgSign=false",
            "-c", "tag.gpgSign=false", *args,
        ], cwd=self.repo, check=True, capture_output=True, text=True).stdout

    def commit(self, tag):
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "synthetic fixture")
        self.git("tag", tag)

    def compare(self, target="v0.37.1"):
        return monitor.compare_source(self.source, "v0.37.0", target, self.output, self.repo.as_uri())

    def test_added_modified_deleted_files_and_scope(self):
        result = self.compare()
        self.assertEqual(result["status"], "changed")
        self.assertEqual(result["changes"], ["A\tpkg/describe/added.go", "M\tpkg/describe/describe.go", "D\tpkg/describe/removed.go"])
        self.assertNotEqual(result["baseline_commit"], result["target_commit"])
        diff = (self.output / result["patch"]).read_text()
        self.assertIn("+new implementation", diff)
        self.assertNotIn("outside tracked scope", diff)
        report = {"baseline": "v0.37.0", "target": "v0.37.1", "status": "changed", "sources": [result]}
        with patch.dict("os.environ", {"GITHUB_STEP_SUMMARY": str(self.root / "step.md")}):
            monitor.write_report(report, self.output)
        self.assertEqual(json.loads((self.output / "report.json").read_text()), report)
        self.assertIn(result["baseline_commit"], (self.root / "step.md").read_text())

    def test_equal_versions_have_no_changes(self):
        result = self.compare("v0.37.0")
        self.assertEqual(result["status"], "unchanged")
        self.assertEqual(result["changes"], [])
        self.assertEqual((self.output / result["patch"]).read_text(), "")

    def test_missing_tag_is_an_error(self):
        with self.assertRaises(RuntimeError):
            self.compare("v0.37.99")

    def test_missing_baseline_path_is_an_error(self):
        self.source["paths"] = ["pkg/typo"]
        with self.assertRaisesRegex(ValueError, "Tracked path missing"):
            self.compare()

    def test_older_target_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "older"):
            self.compare("v0.36.0")


if __name__ == "__main__":
    unittest.main()
