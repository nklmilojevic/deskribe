#!/usr/bin/env python3
"""Report changes in tracked Kubernetes sources without building or running Go."""

import argparse
import datetime
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.compile(r"v0\.(\d+)\.(\d+)")


def version_key(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f"Expected a stable module tag like v0.37.0, got {value!r}")
    return tuple(map(int, match.groups()))


def git(*args, cwd=None):
    result = subprocess.run(
        ["git", "-c", "core.quotePath=true", *args],
        cwd=cwd, capture_output=True, text=True, timeout=180,
        env={**os.environ, "GIT_TERMINAL_PROMPT": "0"},
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"git exited {result.returncode}")
    return result.stdout


def latest_stable(refs):
    tags = []
    for line in refs.splitlines():
        fields = line.split()
        if len(fields) != 2 or not fields[1].startswith("refs/tags/"):
            continue
        tag = fields[1].removeprefix("refs/tags/")
        if VERSION.fullmatch(tag):
            tags.append(tag)
    if not tags:
        raise ValueError("No stable module tags found")
    return max(tags, key=version_key)


def load_config(path):
    config = json.loads(path.read_text())
    version_key(config["baseline"])
    if not config["sources"]:
        raise ValueError("At least one upstream source is required")
    repositories = set()
    for source in config["sources"]:
        repo = source["repository"]
        if not re.fullmatch(r"kubernetes/[a-z][a-z-]+", repo) or repo in repositories:
            raise ValueError(f"Invalid or duplicate repository: {repo!r}")
        repositories.add(repo)
        if not source["paths"]:
            raise ValueError(f"No paths configured for {repo}")
        for path in source["paths"]:
            parts = PurePosixPath(path).parts
            if (not re.fullmatch(r"[a-zA-Z0-9_./-]+", path) or not parts
                    or path.startswith("/") or ".." in parts):
                raise ValueError(f"Invalid tracked path: {path!r}")
    return config


def compare_source(source, baseline, target, output, url=None):
    """Fetch only the two snapshots. The optional URL supports offline Git tests."""
    version_key(baseline)
    if version_key(target) < version_key(baseline):
        raise ValueError("Target version is older than the reviewed baseline")
    repository = source["repository"]
    url = url or f"https://github.com/{repository}.git"
    with tempfile.TemporaryDirectory(prefix="deskribe-upstream-") as temp:
        git("init", "--quiet", temp)
        refs = [f"refs/tags/{tag}:refs/tags/{tag}" for tag in dict.fromkeys([baseline, target])]
        git("fetch", "--quiet", "--no-tags", "--depth=1", url, *refs, cwd=temp)
        before = git("rev-parse", f"{baseline}^{{commit}}", cwd=temp).strip()
        after = git("rev-parse", f"{target}^{{commit}}", cwd=temp).strip()
        for path in source["paths"]:
            if not git("ls-tree", "-r", "--name-only", before, "--", path, cwd=temp).strip():
                raise ValueError(f"Tracked path missing from {baseline}: {repository}/{path}")
        args = ["diff", "--no-ext-diff", "--no-textconv", "--no-renames"]
        changes = git(*args, "--name-status", before, after, "--", *source["paths"], cwd=temp)
        patch = git(*args, before, after, "--", *source["paths"], cwd=temp)
    filename = repository.replace("/", "-") + ".diff"
    (output / filename).write_text(patch)
    return {
        "repository": repository,
        "status": "changed" if changes else "unchanged",
        "baseline_commit": before,
        "target_commit": after,
        "paths": source["paths"],
        "changes": changes.splitlines(),
        "patch": filename,
        "compare_url": f"https://github.com/{repository}/compare/{before}...{after}",
    }


def check(config, target, output):
    report = {
        "schema_version": 1,
        "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "baseline": config["baseline"],
        "target": target or None,
        "status": "error",
        "sources": [],
    }
    try:
        if not target:
            target = latest_stable(git("ls-remote", "--tags", "--refs", "https://github.com/kubernetes/kubectl.git"))
        if version_key(target) < version_key(config["baseline"]):
            raise ValueError("Target version is older than the reviewed baseline")
        report["target"] = target
        for source in config["sources"]:
            try:
                report["sources"].append(compare_source(source, config["baseline"], target, output))
            except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
                report["sources"].append({"repository": source["repository"], "status": "error", "error": str(error)})
        statuses = {source["status"] for source in report["sources"]}
        report["status"] = "error" if "error" in statuses else "changed" if "changed" in statuses else "unchanged"
    except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
        report["error"] = str(error)
    return report


def write_report(report, output):
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    lines = ["# Upstream source check", "", f"Status: **{report['status']}**", "",
             f"Baseline: `{report['baseline']}`; target: `{report['target'] or 'unresolved'}`.", "",
             "This checks tracked source files, not Rust output parity. Changes require human review.", ""]
    if "error" in report:
        lines += ["Error:", "", "```text", report["error"], "```", ""]
    for source in report["sources"]:
        lines += [f"## {source['repository']}: {source['status']}", ""]
        if source["status"] == "error":
            lines += ["```text", source["error"], "```", ""]
        else:
            lines += [f"[Compare commits]({source['compare_url']})", "",
                      f"Baseline commit: `{source['baseline_commit']}`  ",
                      f"Target commit: `{source['target_commit']}`", "",
                      "Tracked paths: " + ", ".join(f"`{path}`" for path in source["paths"]), ""]
            if source["changes"]:
                lines += ["```text", *source["changes"], "```", "", f"Patch: `{source['patch']}` in the report artifact.", ""]
    lines += ["After review, update the Rust implementation and tests as needed, then advance `upstream.json` in a reviewed change.", ""]
    summary = "\n".join(lines)
    (output / "summary.md").write_text(summary)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as file:
            file.write(summary)


def exit_code(report, fail_on_changes):
    if report["status"] == "error":
        return 1
    return 3 if fail_on_changes and report["status"] == "changed" else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=ROOT / "upstream.json")
    parser.add_argument("--target", default="", help="Stable module tag; defaults to the latest kubectl stable tag")
    parser.add_argument("--output", type=Path, required=True, help="Report directory (use a new directory for each run)")
    parser.add_argument("--fail-on-changes", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    try:
        config = load_config(args.config)
    except (OSError, ValueError, KeyError, TypeError) as error:
        report = {"schema_version": 1, "status": "error", "baseline": "invalid config", "target": None, "sources": [], "error": str(error)}
    else:
        report = check(config, args.target, args.output)
    write_report(report, args.output)
    print(f"Upstream check: {report['status']}; report: {args.output / 'summary.md'}")
    return exit_code(report, args.fail_on_changes)


if __name__ == "__main__":
    raise SystemExit(main())
