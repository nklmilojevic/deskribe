# Upstream maintenance

Deskribe follows selected Kubernetes describers and helpers. We review upstream
source changes and port relevant behavior into Rust. We do not build Go or use
upstream output as a live test oracle.

## What the workflow checks

[`upstream.json`](../upstream.json) defines a reviewed baseline tag and source
paths in three repositories:

- `kubernetes/kubectl`: describers and deployment, event, QoS, and RBAC helpers.
- `kubernetes/component-helpers`: resource accounting.
- `kubernetes/apimachinery`: quantities and duration formatting.

The starting baseline is `v0.37.0`. This is a **module tag**, corresponding to
Kubernetes/kubectl release `v1.37.0`. It is a source review baseline, not a claim
that every behavior has been ported or that output is byte-for-byte identical.

The workflow runs on Mondays at 06:00 UTC and through **Actions → Upstream
changes → Run workflow**. Leave the target empty to select the highest stable
kubectl module tag. Alpha, beta, and release-candidate tags are ignored. The same
tag must exist in all tracked repositories; a missing tag is an error.

Each comparison fetches two shallow Git snapshots and compares only the tracked
paths. Added, modified, and deleted files are included, along with tests under
those paths. A missing baseline path is an error rather than an empty comparison.
The report records resolved commit IDs for both tags. Tags are resolved on each
run, not independently verified against stored commit hashes.

## Results

The workflow publishes a job summary and an `upstream-report` artifact, retained
for 30 days. The artifact contains:

- `summary.md`: status, tracked paths, changed files, and upstream links.
- `report.json`: machine-readable status and commit IDs.
- One `.diff` file per successfully compared repository, including empty patches
  when no tracked files changed.

Source changes deliberately fail the comparison step so the run needs attention.
A fetch, timeout, or configuration error also fails the step, but the report labels
it `error`, not `changed` or `unchanged`. Reports from successful comparisons are
kept even if another repository fails.

The script's exit codes are `0` for a successful check, `1` for a check error, and
`3` for changes when `--fail-on-changes` is used. Invalid command arguments use
Python argparse's exit code `2`.

The workflow has read-only repository permissions. It does not open issues,
change code, advance the baseline, publish packages, or run Go. Pull requests
that change the monitor run its offline tests, not the network comparison.
Scheduled runs require this workflow on the repository's default branch. Normal
GitHub Actions scheduling and notification settings apply; GitHub may delay runs
or disable schedules in inactive public repositories.

## Run locally

Requirements: Python 3.9 or newer, Git, and access to public GitHub repositories.
No Python packages or GitHub token are required.

```sh
python3 scripts/check_upstream.py --output upstream-report
# Or select a stable module tag explicitly:
python3 scripts/check_upstream.py --target v0.37.0 --output baseline-report
```

Choose a new output directory for each run; existing directories are not
reused or deleted. Add `--fail-on-changes` to use the workflow's exit behavior.

Test the monitor without network access:

```sh
python3 -m unittest discover -s scripts -p 'test_check_upstream.py' -v
```

These tests use temporary local Git repositories and mocked fetch failures.

## Review an update

1. Read the report and patches. A source change may be a refactor, a test change,
   or a behavior change; it does not automatically require a Rust change.
2. Follow changed imports and helpers. Update the tracked paths if upstream moves
   code or starts using a new helper. Changes outside the tracked paths are not
   covered by this workflow.
3. Port relevant behavior and add Rust tests. Preserve fresh reads, UID checks,
   cancellation, older API fallbacks, sensitive-data handling, and optional reads.
4. Run the Rust tests, clippy, and formatting checks. Review any snapshot updates
   independently; regenerated output is not proof of correctness. Test sofka's
   integration separately when its behavior could be affected.
5. Advance `baseline` in `upstream.json` to the reviewed target, in the same
   reviewed change. Explain any intentionally unported differences. Do not move
   the baseline only to make the workflow green.

The monitor detects source changes, not behavioral parity, dependency API changes
outside its scope, or all changes on older supported release branches. It compares
the newest stable tag with the baseline; it does not poll every maintenance branch.
