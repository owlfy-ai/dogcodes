#!/bin/sh
set -eu

: "${DOGCODES_DEV_SIGNING_IDENTITY:?Missing development signing identity}"
: "${DOGCODES_DEV_SIGNING_IDENTIFIER:?Missing development signing identifier}"
: "${1:?Missing application executable}"

# Development signatures need no notarization/network timestamp. Keep a stable
# identifier and certificate requirement instead of the linker's cdhash identity.
/usr/bin/codesign --force --sign "$DOGCODES_DEV_SIGNING_IDENTITY" \
  --identifier "$DOGCODES_DEV_SIGNING_IDENTIFIER" --timestamp=none "$1"
/usr/bin/codesign --verify --strict "$1"

# Cargo/Tauri must own the actual application, including during hot reload.
exec "$@"
