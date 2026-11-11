#!/bin/sh
# Fabrique dist/neofrag-linux-x86_64.tar.gz : binaire compatible glibc >= 2.28
# (Debian 10+, Ubuntu 18.10+, RHEL 8+), fichiers statiques et aides au déploiement.
# Prérequis : cargo-zigbuild (cargo install cargo-zigbuild) et zig (pip install ziglang).
set -e
cd "$(dirname "$0")/.."
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28
OUT=dist/neofrag
rm -rf dist && mkdir -p "$OUT"
cp target/x86_64-unknown-linux-gnu/release/neofrag "$OUT/"
cp -r assets "$OUT/assets"

cp neofrag.example.toml COPYING COPYING.LESSER "$OUT/"
cp packaging/LISEZ-MOI.txt packaging/start.sh packaging/neofrag.service packaging/nginx.conf.example \
   packaging/egg-neofrag.json packaging/docker-compose.yml "$OUT/"
chmod +x "$OUT/neofrag" "$OUT/start.sh"
tar -C dist -czf dist/neofrag-linux-x86_64.tar.gz neofrag
ls -lh dist/neofrag-linux-x86_64.tar.gz
