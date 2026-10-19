#!/bin/sh
# Guards the kit's image archive against the one shape Docker 20.10.24
# refuses to load: an attestation manifest (buildx provenance/SBOM,
# in-toto) alongside the image manifests. A modern `docker save` also
# writes index.json and oci-layout next to manifest.json — that is normal
# and the old engine ignores both, so this script does not check for them.
set -eu

archive="${1:?usage: check-image-archive.sh <archive.tar.gz>}"

if [ ! -f "$archive" ]; then
    echo "check-image-archive: no such file: $archive" >&2
    exit 1
fi

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

if ! tar -tzf "$archive" >"$tmpdir/list" 2>"$tmpdir/list.err"; then
    echo "check-image-archive: cannot read $archive as a tar.gz archive" >&2
    cat "$tmpdir/list.err" >&2
    exit 1
fi

manifest_entry=$(grep -E '(^|/)manifest\.json$' "$tmpdir/list" | head -1)
if [ -z "$manifest_entry" ]; then
    echo "check-image-archive: manifest.json not found in $archive" >&2
    exit 1
fi
tar -xzf "$archive" -O "$manifest_entry" >"$tmpdir/manifest.json"

# index.json (the OCI index docker save adds alongside manifest.json) is
# where an attestation manifest's vnd.docker.reference.type annotation
# usually lives; check it too when present, though it is not required to
# exist.
index_entry=$(grep -E '(^|/)index\.json$' "$tmpdir/list" | head -1 || true)
files="$tmpdir/manifest.json"
if [ -n "$index_entry" ]; then
    tar -xzf "$archive" -O "$index_entry" >"$tmpdir/index.json"
    files="$files $tmpdir/index.json"
fi

hit=$(grep -l -E 'vnd\.docker\.reference\.type|application/vnd\.in-toto' $files 2>/dev/null | head -1 || true)
if [ -n "$hit" ]; then
    echo "check-image-archive: attestation manifest found in $archive (in $(basename "$hit")) — Docker 20.10.24 refuses to load an archive that carries one" >&2
    exit 1
fi

echo "check-image-archive: $archive is clean (manifest.json present, no attestation manifests)"
