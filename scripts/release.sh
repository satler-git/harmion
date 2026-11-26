#!/usr/bin/env bash
# Bump a version and create the release tag.
#
#   scripts/release.sh native 0.2.0
#   scripts/release.sh android mote 0.2.0
#   scripts/release.sh android mote 0.2.0 --version-code 7
#
# The script commits the version bump and creates an annotated tag, but does
# not push. Use --push to push HEAD and the tag.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

die() {
    echo "release: $*" >&2
    exit 1
}

push=0
version_code_override=""
args=()
while (($# > 0)); do
    case "$1" in
        --push)
            push=1
            shift
            ;;
        --version-code)
            (($# >= 2)) || die "--version-code needs a value"
            version_code_override="$2"
            shift 2
            ;;
        --help|-h)
            sed -n '2,12p' "$0"
            exit 0
            ;;
        *)
            args+=("$1")
            shift
            ;;
    esac
done

[[ -z "$(git status --porcelain)" ]] || die "working tree is not clean"

lane="${args[0]:-}"
case "$lane" in
    native)
        ((${#args[@]} == 2)) || die "usage: release.sh native VERSION"
        version="${args[1]}"
        [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] \
            || die "invalid semver version: $version"
        tag="v$version"
        command -v cargo >/dev/null 2>&1 || die "cargo is required"
        perl -0pi -e "s/^version = \"[^\"]+\"\$/version = \"$version\"/m" Cargo.toml
        cargo update --workspace
        git add Cargo.toml Cargo.lock
        git commit -m "chore(release): native v$version"
        ;;
    android)
        ((${#args[@]} == 3)) || die "usage: release.sh android APP VERSION"
        app="${args[1]}"
        version="${args[2]}"
        [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] \
            || die "invalid semver version: $version"
        command -v python3 >/dev/null 2>&1 || die "python3 is required"
        readarray -t fields < <(
            python3 - "$app" <<'PY'
import sys, tomllib
app_name = sys.argv[1]
with open("android/release-apps.toml", "rb") as fh:
    data = tomllib.load(fh)
for app in data.get("app", []):
    if app["module"] == app_name:
        print(app["version_file"])
        print(app.get("tag_prefix", ""))
        print("true" if app.get("ship") else "false")
        break
else:
    raise SystemExit(f"unknown app: {app_name}")
PY
        )
        version_file="${fields[0]}"
        tag_prefix="${fields[1]}"
        ship="${fields[2]}"
        [[ "$ship" == "true" ]] || die "$app is not shippable"
        current_code="$(sed -n 's/^versionCode=//p' "$version_file" | tail -n1)"
        [[ "$current_code" =~ ^[0-9]+$ ]] || die "invalid versionCode in $version_file"
        version_code="${version_code_override:-$((current_code + 1))}"
        printf 'versionCode=%s\nversionName=%s\n' "$version_code" "$version" > "$version_file"
        tag="${tag_prefix}${version}"
        git add "$version_file"
        git commit -m "chore(release): $app v$version"
        ;;
    *)
        die "usage: release.sh native VERSION | release.sh android APP VERSION"
        ;;
esac

git tag -a "$tag" -m "$tag"
echo "created $tag"
if ((push)); then
    git push origin HEAD
    git push origin "$tag"
fi
