set unstable
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

root := justfile_directory()

[private]
default:
    @just --list release

# Release a patch version.
patch:
    just --justfile "{{ root }}/Justfile" release _release patch

# Release a minor version.
minor:
    just --justfile "{{ root }}/Justfile" release _release minor

# Release a major version.
major:
    just --justfile "{{ root }}/Justfile" release _release major

[private]
_release bump:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{ root }}"
    bump="{{ bump }}"
    case "$bump" in patch|minor|major) ;; *) echo "unknown bump '$bump'" >&2; exit 1 ;; esac
    if command -v op >/dev/null 2>&1; then
      gh() { op plugin run -- gh "$@"; }
    else
      gh() { command gh "$@"; }
    fi
    just --justfile "{{ root }}/Justfile" release _ensure-release-ready
    latest="$(gh release list --exclude-drafts --exclude-pre-releases --limit 1 --json tagName --jq '.[0].tagName // empty')"
    current="$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])')"
    if test -z "$latest"; then latest="v$current"; fi
    version="$(just --justfile "{{ root }}/Justfile" release _next-version "$latest" "$bump")"
    if test "$current" != "${latest#v}" && test "$current" != "$version"; then
      echo "Cargo version '$current' must match '$latest' or the next version '$version'" >&2
      exit 1
    fi
    new_tag="v$version"
    if git show-ref --verify --quiet "refs/tags/$new_tag"; then
      echo "tag '$new_tag' already exists; inspect the existing release before retrying" >&2
      exit 1
    fi
    echo "$latest -> $new_tag"
    tmp="$(mktemp)"
    trap 'rm -f "$tmp"' EXIT
    awk -v version="$version" '
      /^version = "/ && !done {
        print "version = \"" version "\""
        done = 1
        next
      }
      { print }
      END { if (!done) exit 1 }
    ' Cargo.toml > "$tmp"
    cp "$tmp" Cargo.toml
    K8S_OPENAPI_ENABLED_VERSION=1.36 cargo check --quiet
    just --justfile "{{ root }}/Justfile" check
    K8S_OPENAPI_ENABLED_VERSION=1.36 cargo publish --locked --dry-run --allow-dirty
    git add Cargo.toml Cargo.lock
    if git diff --cached --quiet; then
      echo "Cargo.toml is already at v$version"
    else
      git commit -m "chore(release): v$version"
      git push origin main
    fi
    target="$(git rev-parse HEAD)"
    gh release create "$new_tag" --target "$target" --generate-notes

[private]
_ensure-release-ready:
    #!/usr/bin/env bash
    set -euo pipefail
    cd "{{ root }}"
    if command -v op >/dev/null 2>&1; then
      gh() { op plugin run -- gh "$@"; }
    else
      gh() { command gh "$@"; }
    fi
    test "$(git branch --show-current)" = main || { echo "release from main only" >&2; exit 1; }
    test -z "$(git status --porcelain)" || { echo "working tree and index must be clean" >&2; exit 1; }
    gh auth status >/dev/null
    git fetch origin main --tags
    git pull --ff-only origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" || { echo "local main must match origin/main" >&2; exit 1; }

[private]
_next-version latest bump:
    #!/usr/bin/env bash
    set -euo pipefail
    latest="{{ latest }}"
    bump="{{ bump }}"
    latest="${latest#v}"
    if ! [[ "$latest" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
      echo "cannot parse version '$latest'" >&2
      exit 1
    fi
    IFS=. read -r major minor patch <<< "$latest"
    case "$bump" in
      major) major=$((major + 1)); minor=0; patch=0 ;;
      minor) minor=$((minor + 1)); patch=0 ;;
      patch) patch=$((patch + 1)) ;;
      *) echo "unknown bump '$bump'" >&2; exit 1 ;;
    esac
    echo "$major.$minor.$patch"
