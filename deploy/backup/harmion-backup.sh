#!/usr/bin/env bash
# Encrypted offsite snapshot of a Harmion node.
#
# Requires HARMION_BACKUP_KEY (passphrase file, 0600), HARMION_BACKUP_KV
# (Workers KV namespace id), CLOUDFLARE_ACCOUNT_ID and HARMION_CF_OAUTH (a
# 0600 JSON file with access_token, refresh_token and expires_at). The database
# is copied with SQLite's online VACUUM INTO path, so the daemon may keep
# running. The archive is encrypted before it leaves the host.
set -euo pipefail

node=${1:?usage: harmion-backup NODE_DIR}
parent=$(mktemp -d "${TMPDIR:-/tmp}/harmion-backup.XXXXXX")
: "${HARMION_BACKUP_KEY:?set HARMION_BACKUP_KEY to an owner-only passphrase file}"
: "${HARMION_BACKUP_KV:?set HARMION_BACKUP_KV to a Workers KV namespace id}"
: "${CLOUDFLARE_ACCOUNT_ID:?set CLOUDFLARE_ACCOUNT_ID}"
: "${HARMION_CF_OAUTH:?set HARMION_CF_OAUTH to the OAuth credential JSON}"
harmiond=${HARMIOND:-harmiond}

umask 077
trap 'rm -rf "$parent"' EXIT

cf_token() {
  local now expires access refresh response
  now=$(date +%s)
  expires=$(jq -r '.expires_at' "$HARMION_CF_OAUTH")
  if [ "$expires" -gt $((now + 120)) ]; then
    jq -r '.access_token' "$HARMION_CF_OAUTH"
    return
  fi
  refresh=$(jq -r '.refresh_token' "$HARMION_CF_OAUTH")
  response=$(curl -fsS -X POST https://dash.cloudflare.com/oauth2/token \
    -H 'Content-Type: application/x-www-form-urlencoded' \
    --data-urlencode grant_type=refresh_token \
    --data-urlencode client_id=54d11594-84e4-41aa-b438-e81b8fa78ee7 \
    --data-urlencode refresh_token="$refresh")
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

key="main/$(date -u +%Y%m%dT%H%M%SZ).tar.gpg"
encoded=$(jq -rn --arg key "$key" '$key | @uri')
token=$(cf_token)
curl -fsS -X PUT \
  -H "Authorization: Bearer $token" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @"$encrypted" \
  "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/storage/kv/namespaces/$HARMION_BACKUP_KV/values/$encoded"
printf 'uploaded %s\n' "$key"
