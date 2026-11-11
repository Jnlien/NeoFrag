#!/bin/sh
# Lance NeoFrag depuis le dossier où se trouve ce script.
cd "$(dirname "$0")" || exit 1
chmod +x ./neofrag 2>/dev/null
exec ./neofrag "$@"
