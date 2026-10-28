#!/bin/sh
# Use the client modules paired with this AppImage, including after relocation.
dogcodes_appdir="${APPDIR:-$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)}"
export SPA_PLUGIN_DIR="$dogcodes_appdir/usr/lib/spa-0.2"
export PIPEWIRE_MODULE_DIR="$dogcodes_appdir/usr/lib/pipewire-0.3"
export PIPEWIRE_CONFIG_DIR="$dogcodes_appdir/usr/share/dog.codes/pipewire"
export PIPEWIRE_CONFIG_NAME=client.conf
exec "$dogcodes_appdir/usr/bin/desktop" "$@"
