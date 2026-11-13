#!/bin/bash
# Download one known artifact across mirrors; publish only verified bytes.
# Callers supply the independently audited SHA-256, never one from this request.
set -euo pipefail
expected="${1:-}"
destination="${2:?destination required}"
shift 2
if [[ ! "$expected" =~ ^[0-9a-fA-F]{64}$ ]] || [ "$#" -eq 0 ]; then
    echo 'ERROR: an independently verified SHA-256 and artifact URL are required.' >&2
    exit 1
fi
partial="$(mktemp "$destination.part.XXXXXX")"
trap 'rm -f "$partial"' EXIT
for url in "$@"; do
    if curl -sSfL --proto '=https' --proto-redir '=https' --retry 3 --retry-delay 2 -o "$partial" "$url"; then
        digest="$(sha256sum "$partial" | awk '{print $1}')"
        if [ "$digest" = "${expected,,}" ]; then
            # Docker's unprivileged runtime must be able to read the plugin.
            chmod 644 "$partial"
            mv -f "$partial" "$destination"
            exit 0
        fi
        echo "ERROR: SHA-256 mismatch for $url (expected $expected, got $digest)." >&2
    fi
done
echo "ERROR: no mirror supplied the verified artifact for $destination." >&2
exit 1
