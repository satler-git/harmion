#!/usr/bin/env bash
# Encrypted offsite snapshot of a Harmion node.
#
# Requires HARMION_BACKUP_KEY (passphrase file, 0600) and HARMION_BACKUP_KV
# (Workers KV namespace id). The database is copied with SQLite's online
# VACUUM INTO path, so the daemon may keep running. The archive is encrypted
# before it leaves the host; the passphrase file must be kept offline.
set -euo pipefail

node=${1:?usage: harmion-backup NODE_DIR}
parent=$(mktemp -d "${TMPDIR:-/tmp}/harmion-backup.XXXXXX")
: "${HARMION_BACKUP_KEY:?set HARMION_BACKUP_KEY to an owner-only passphrase file}"
: "${HARMION_BACKUP_KV:?set HARMION_BACKUP_KV to a Workers KV namespace id}"
harmiond=${HARMIOND:-harmiond}
wrangler=${WRANGLER:-wrangler}

umask 077
trap 'rm -rf "$parent"' EXIT

"$harmiond" snapshot "$node" "$parent/snapshot"
archive="$parent/backup.tar"
encrypted="$parent/backup.tar.gpg"
tar -C "$parent/snapshot" -cf "$archive" .
gpg --batch --yes --quiet --symmetric --cipher-algo AES256 \
  --passphrase-file "$HARMION_BACKUP_KEY" --output "$encrypted" "$archive"

key="main/$(date -u +%Y%m%dT%H%M%SZ).tar.gpg"
"$wrangler" kv key put "$key" --path "$encrypted" \
  --namespace-id "$HARMION_BACKUP_KV" --remote
printf 'uploaded %s\n' "$key"
