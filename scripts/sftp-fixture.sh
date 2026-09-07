#!/usr/bin/env bash
# A real OpenSSH SFTP server for the phase 0 compatibility prototypes (§0.6).
#
# The point of this fixture is that its answers are checkable: fixed host keys, so a
# changed-key test is possible and fingerprints are stable across restarts; a password
# user and two key users, one of whose keys is passphrase-protected; and a data
# directory mounted from the host so uploads can be verified outside the client under
# test.
#
#   scripts/sftp-fixture.sh up      start it and print the environment
#   scripts/sftp-fixture.sh env     print the environment for an already-running one
#   scripts/sftp-fixture.sh rotate  swap in a different host key (changed-key test)
#   scripts/sftp-fixture.sh down    stop and remove it
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/target/sftp-fixture"
NAME="relay-sftp-fixture"
IMAGE="atmoz/sftp:alpine"
PORT="${RELAY_SFTP_PORT:-22022}"
USER_NAME="deploy"
PASSWORD="hunter2"
PASSPHRASE="correct horse"

keys() {
  mkdir -p "$DIR/keys" "$DIR/data" "$DIR/hostkeys"

  [ -f "$DIR/keys/id_ed25519" ] ||
    ssh-keygen -q -t ed25519 -N '' -C relay-fixture -f "$DIR/keys/id_ed25519"
  # An encrypted key: phase 0 must prove the passphrase prompt path, not just the
  # happy one.
  [ -f "$DIR/keys/id_ed25519_locked" ] ||
    ssh-keygen -q -t ed25519 -N "$PASSPHRASE" -C relay-fixture-locked \
      -f "$DIR/keys/id_ed25519_locked"
  [ -f "$DIR/keys/id_rsa" ] ||
    ssh-keygen -q -t rsa -b 3072 -N '' -C relay-fixture-rsa -f "$DIR/keys/id_rsa"

  # Two host keys so `rotate` can present a different one to the same host:port.
  [ -f "$DIR/hostkeys/primary" ] ||
    ssh-keygen -q -t ed25519 -N '' -C relay-fixture-host -f "$DIR/hostkeys/primary"
  [ -f "$DIR/hostkeys/rotated" ] ||
    ssh-keygen -q -t ed25519 -N '' -C relay-fixture-host-2 -f "$DIR/hostkeys/rotated"
  chmod 600 "$DIR/hostkeys/primary" "$DIR/hostkeys/rotated"
}

seed() {
  mkdir -p "$DIR/data/assets" "$DIR/data/empty-dir"
  printf '<!doctype html><title>relay fixture</title>\n' > "$DIR/data/index.html"
  # Sizes chosen to exercise different paths: under one chunk, several chunks, and
  # large enough that a cancellation lands mid-transfer.
  head -c 4096 /dev/urandom > "$DIR/data/assets/small.bin"
  head -c 1048576 /dev/urandom > "$DIR/data/assets/one-mib.bin"
  head -c 33554432 /dev/urandom > "$DIR/data/assets/thirty-two-mib.bin"
  : > "$DIR/data/assets/zero-bytes"
}

host_key_file() { echo "$DIR/hostkeys/${1:-primary}"; }

start() {
  local which_key="${1:-primary}"
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" \
    -p "127.0.0.1:$PORT:22" \
    -v "$DIR/keys/id_ed25519.pub:/home/$USER_NAME/.ssh/keys/id_ed25519.pub:ro" \
    -v "$DIR/keys/id_ed25519_locked.pub:/home/$USER_NAME/.ssh/keys/id_ed25519_locked.pub:ro" \
    -v "$DIR/keys/id_rsa.pub:/home/$USER_NAME/.ssh/keys/id_rsa.pub:ro" \
    -v "$(host_key_file "$which_key"):/etc/ssh/ssh_host_ed25519_key:ro" \
    -v "$DIR/data:/home/$USER_NAME/upload" \
    "$IMAGE" "$USER_NAME:$PASSWORD:$(id -u):$(id -g)" >/dev/null

  for _ in $(seq 1 60); do
    if docker logs "$NAME" 2>&1 | grep -q "Server listening on 0.0.0.0"; then return 0; fi
    sleep 0.5
  done
  echo "fixture did not come up:" >&2
  docker logs "$NAME" >&2
  exit 1
}

fingerprint() {
  ssh-keygen -lf "$(host_key_file "${1:-primary}").pub" | awk '{print $2}'
}

print_env() {
  cat <<ENV
export RELAY_SFTP_HOST=127.0.0.1
export RELAY_SFTP_PORT=$PORT
export RELAY_SFTP_USER=$USER_NAME
export RELAY_SFTP_PASSWORD=$PASSWORD
export RELAY_SFTP_KEY=$DIR/keys/id_ed25519
export RELAY_SFTP_KEY_LOCKED=$DIR/keys/id_ed25519_locked
export RELAY_SFTP_KEY_PASSPHRASE='$PASSPHRASE'
export RELAY_SFTP_KEY_RSA=$DIR/keys/id_rsa
export RELAY_SFTP_HOSTKEY_FP='$(fingerprint primary)'
export RELAY_SFTP_HOSTKEY_FP_ROTATED='$(fingerprint rotated)'
export RELAY_SFTP_DATA=$DIR/data
ENV
}

case "${1:-up}" in
  up)
    keys
    seed
    start primary
    print_env
    ;;
  env) print_env ;;
  rotate)
    keys
    start rotated
    echo "# now presenting $(fingerprint rotated)" >&2
    ;;
  restore)
    keys
    start primary
    ;;
  down)
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    echo "stopped $NAME" >&2
    ;;
  *)
    echo "usage: $0 {up|env|rotate|restore|down}" >&2
    exit 2
    ;;
esac
