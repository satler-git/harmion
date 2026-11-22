#!/usr/bin/env bash
# Encrypted offsite snapshot of a Harmion node.
#
# Requires HARMION_BACKUP_KEY (passphrase file, 0600), HARMION_BACKUP_KV
# (Workers KV namespace id) and CLOUDFLARE_ACCOUNT_ID, plus a Cloudflare
# credential, tried in this order:
#   1. HARMION_CF_TOKEN_OP_REF - an `op read` reference such as
#      op://Private/Cloudflare-KV/credential (1Password CLI, app unlocked).
#   2. HARMION_CF_TOKEN - a 0600 file holding a KV-scoped API token.
#   3. HARMION_CF_OAUTH - OAuth JSON. A refresh token shared with wrangler is
#      invalidated by rotation/reuse detection, so this is the last resort.
# HARMION_OP_SERVICE_ACCOUNT_TOKEN (0600 file) is optional and lets `op` run
# headless with a service account instead of the desktop app integration.
# The database is copied with SQLite's online VACUUM INTO path, so the daemon
# may keep running. The archive is encrypted before it leaves the host.
#
# Optional: HARMION_BACKUP_KEEP (generations to retain, default 14).
#
# The KV object prefix is the node directory name, so several nodes backed up
# by the same namespace do not overwrite or restore each other's archives.
set -euo pipefail

node=${1:?usage: harmion-backup NODE_DIR}
prefix=$(basename "$node")
case "$prefix" in
  '' | *[!A-Za-z0-9_-]*)
    echo "harmion-backup: node directory name must be [A-Za-z0-9_-]" >&2
    exit 2
    ;;
esac

umask 077
parent=$(mktemp -d "${TMPDIR:-/tmp}/harmion-backup.XXXXXX")
trap 'rm -rf "$parent"' EXIT

: "${HARMION_BACKUP_KEY:?set HARMION_BACKUP_KEY to an owner-only passphrase file}"
: "${HARMION_BACKUP_KV:?set HARMION_BACKUP_KV to a Workers KV namespace id}"
: "${CLOUDFLARE_ACCOUNT_ID:?set CLOUDFLARE_ACCOUNT_ID}"
harmiond=${HARMIOND:-harmiond}
keep=${HARMION_BACKUP_KEEP:-14}
case "$keep" in
  '' | *[!0-9]*)
    echo "harmion-backup: HARMION_BACKUP_KEEP must be a non-negative integer" >&2
    exit 2
    ;;
esac
kv_value_limit=26214400 # Workers KV: 25 MiB per value.
kv_url="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/storage/kv/namespaces/$HARMION_BACKUP_KV"

require_owner_only() {
  local file=$1 mode
  if [ ! -r "$file" ]; then
    echo "harmion-backup: $file is not readable" >&2
    exit 1
  fi
  mode=$(stat -c %a "$file")
  if [ $((mode % 100)) -ne 0 ]; then
    echo "harmion-backup: refusing group/other-accessible $file (mode $mode)" >&2
    exit 1
  fi
}
require_owner_only "$HARMION_BACKUP_KEY"
if [ -n "${HARMION_OP_SERVICE_ACCOUNT_TOKEN:-}" ]; then
  require_owner_only "$HARMION_OP_SERVICE_ACCOUNT_TOKEN"
fi
if [ -n "${HARMION_CF_TOKEN_OP_REF:-}" ]; then
  # 1Password is primary; a token file is an optional fallback.
  if [ -n "${HARMION_CF_TOKEN:-}" ] && [ -r "${HARMION_CF_TOKEN}" ]; then
    require_owner_only "$HARMION_CF_TOKEN"
  fi
elif [ -n "${HARMION_CF_TOKEN:-}" ]; then
  require_owner_only "$HARMION_CF_TOKEN"
elif [ -n "${HARMION_CF_OAUTH:-}" ]; then
  require_owner_only "$HARMION_CF_OAUTH"
else
  echo "harmion-backup: set HARMION_CF_TOKEN_OP_REF, HARMION_CF_TOKEN, or HARMION_CF_OAUTH" >&2
  exit 1
fi

# Never put tokens on argv: /proc/<pid>/cmdline is readable by other local
# users even when the files themselves are 0600. Bodies go through stdin and
# headers through `curl --config -`.
cf_token() {
  local now expires access refresh body response
  if [ -n "${HARMION_CF_TOKEN_OP_REF:-}" ]; then
    if ! command -v op >/dev/null 2>&1; then
      echo "harmion-backup: op is not on PATH; cannot read $HARMION_CF_TOKEN_OP_REF" >&2
    else
      if [ -n "${HARMION_OP_SERVICE_ACCOUNT_TOKEN:-}" ]; then
        if ! OP_SERVICE_ACCOUNT_TOKEN=$(tr -d '\r\n' < "$HARMION_OP_SERVICE_ACCOUNT_TOKEN"); then
          echo "harmion-backup: cannot read $HARMION_OP_SERVICE_ACCOUNT_TOKEN" >&2
          exit 1
        fi
        export OP_SERVICE_ACCOUNT_TOKEN
      fi
      # Bound the call: a locked 1Password app must not hang the daily unit.
      if access=$(timeout 30 op read --no-newline "$HARMION_CF_TOKEN_OP_REF" 2>"$parent/op.err") \
        && [ -n "$access" ]; then
        printf '%s' "$access"
        return
      fi
      reason=$(head -n 1 "$parent/op.err" 2>/dev/null | tr -d '\r' | cut -c1-200)
      echo "harmion-backup: 1Password read failed${reason:+: $reason}" >&2
    fi
    if [ -z "${HARMION_CF_TOKEN:-}" ] || [ ! -r "${HARMION_CF_TOKEN:-/nonexistent}" ]; then
      exit 1
    fi
    echo "harmion-backup: falling back to the HARMION_CF_TOKEN file" >&2
  fi
  if [ -n "${HARMION_CF_TOKEN:-}" ] && [ -r "${HARMION_CF_TOKEN:-/nonexistent}" ]; then
    access=$(tr -d '\r\n' < "$HARMION_CF_TOKEN")
    if [ -z "$access" ]; then
      echo "harmion-backup: $HARMION_CF_TOKEN is empty" >&2
      exit 1
    fi
    printf '%s' "$access"
    return
  fi
  now=$(date +%s)
  expires=$(jq -r '.expires_at // 0' "$HARMION_CF_OAUTH")
  if [ "$expires" -gt $((now + 120)) ]; then
    jq -r '.access_token' "$HARMION_CF_OAUTH"
    return
  fi
  refresh=$(jq -r '.refresh_token' "$HARMION_CF_OAUTH")
  body=$(jq -rn --arg client '54d11594-84e4-41aa-b438-e81b8fa78ee7' --arg rt "$refresh" \
    '"grant_type=refresh_token&client_id=\($client|@uri)&refresh_token=\($rt|@uri)"')
  if ! response=$(printf '%s' "$body" | curl -fsS --connect-timeout 10 --max-time 60 \
    --retry 3 --retry-delay 5 -X POST https://dash.cloudflare.com/oauth2/token \
    -H 'Content-Type: application/x-www-form-urlencoded' --data-binary @-); then
    echo "harmion-backup: Cloudflare OAuth refresh failed; use a KV-scoped HARMION_CF_TOKEN" >&2
    exit 1
  fi
  if ! printf '%s' "$response" | jq -e '.access_token and .refresh_token and (.expires_in | numbers)' >/dev/null; then
    echo "harmion-backup: Cloudflare OAuth token response is invalid; use a KV-scoped HARMION_CF_TOKEN" >&2
    exit 1
  fi
  access=$(printf '%s' "$response" | jq -r '.access_token')
  refresh=$(printf '%s' "$response" | jq -r '.refresh_token')
  expires=$((now + $(printf '%s' "$response" | jq -r '.expires_in')))
  printf '%s' "$response" | jq --arg access "$access" --arg refresh "$refresh" \
    --argjson expires "$expires" \
    '{access_token:$access,refresh_token:$refresh,expires_at:$expires}' \
    > "$HARMION_CF_OAUTH.new"
  chmod 600 "$HARMION_CF_OAUTH.new"
  mv "$HARMION_CF_OAUTH.new" "$HARMION_CF_OAUTH"
  printf '%s' "$access"
}

"$harmiond" snapshot "$node" "$parent/snapshot"
archive="$parent/backup.tar"
encrypted="$parent/backup.tar.gpg"
tar -C "$parent/snapshot" -cf "$archive" .
gpg --batch --yes --quiet --symmetric --cipher-algo AES256 \
  --passphrase-file "$HARMION_BACKUP_KEY" --output "$encrypted" "$archive"

size=$(stat -c %s "$encrypted")
if [ "$size" -gt "$kv_value_limit" ]; then
  echo "harmion-backup: encrypted snapshot is $size bytes; Workers KV allows $kv_value_limit per value" >&2
  exit 1
fi

key="$prefix/$(date -u +%Y%m%dT%H%M%SZ).tar.gpg"
encoded=$(jq -rn --arg key "$key" '$key | @uri')
token=$(cf_token)
if [ -z "$token" ]; then
  echo "harmion-backup: no Cloudflare credential" >&2
  exit 1
fi
curl -fsS --connect-timeout 10 --max-time 900 --retry 3 --retry-delay 5 \
  --request PUT --data-binary @"$encrypted" --config - \
  "$kv_url/values/$encoded" <<EOF
header = "Authorization: Bearer $token"
header = "Content-Type: application/octet-stream"
EOF
printf 'uploaded %s (%s bytes)\n' "$key" "$size"

# Retain only the newest HARMION_BACKUP_KEEP generations for this node. A
# Workers KV value is capped at 25 MiB, so an unlimited daily history both
# fills the namespace and starts failing every later upload.
list_prefix=$(jq -rn --arg prefix "$prefix/" '$prefix | @uri')
listing=$(curl -fsS --connect-timeout 10 --max-time 60 --retry 3 --retry-delay 5 \
  --config - "$kv_url/keys?prefix=$list_prefix&limit=1000" <<EOF
header = "Authorization: Bearer $token"
EOF
)
printf '%s' "$listing" | jq -r '.result[].name' | sort -r | tail -n +$((keep + 1)) |
  while IFS= read -r old; do
    [ -n "$old" ] || continue
    old_encoded=$(jq -rn --arg key "$old" '$key | @uri')
    curl -fsS --connect-timeout 10 --max-time 60 --retry 3 --retry-delay 5 \
      --request DELETE --config - "$kv_url/values/$old_encoded" <<EOF
header = "Authorization: Bearer $token"
EOF
  done
