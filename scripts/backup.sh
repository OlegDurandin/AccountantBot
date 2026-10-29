#!/bin/sh
set -eu

db_path="${1:-data/accountant.db}"
backup_dir="${2:-backups}"

mkdir -p "$backup_dir"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup_path="$backup_dir/accountant-$timestamp.db"
sqlite3 "$db_path" ".backup '$backup_path'"
printf '%s\n' "$backup_path"
