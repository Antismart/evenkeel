#!/usr/bin/env bash
# Phase 0 spike: one-time setup + start for the FNN Pudge-testnet node.
#
# What it does (idempotent — safe to re-run):
#   1. Generates a node private key at fiber-node/ckb/key (if absent).
#   2. Creates .env with a random FIBER_SECRET_KEY_PASSWORD (if absent).
#   3. Seeds fiber-node/config.yml from the v0.8.1 bundled testnet config,
#      patched so the RPC binds 0.0.0.0:8227 *inside the container* — the
#      docker-compose port mapping keeps it loopback-only on the host.
#   4. Starts the node via docker compose.
#
# After it runs: fund the printed testnet address at https://faucet.nervos.org
# (the node needs on-chain CKB to open channels), then follow README.md.
set -euo pipefail
cd "$(dirname "$0")"

FNN_TAG="v0.8.1"
CONFIG_URL="https://raw.githubusercontent.com/nervosnetwork/fiber/${FNN_TAG}/config/testnet/config.yml"

mkdir -p fiber-node/ckb

# 1. Node key. FNN reads a 32-byte hex private key from ckb/key and encrypts
#    it in place on first start using FIBER_SECRET_KEY_PASSWORD.
if [ ! -f fiber-node/ckb/key ]; then
  umask 077
  printf '%s' "$(openssl rand -hex 32)" > fiber-node/ckb/key
  echo "Generated new node key: fiber-node/ckb/key (KEEP OUT OF GIT — .gitignore covers it)"
else
  echo "Node key already present: fiber-node/ckb/key"
fi

# 2. .env for docker compose.
if [ ! -f .env ]; then
  umask 077
  printf 'FIBER_SECRET_KEY_PASSWORD=%s\n' "$(openssl rand -hex 16)" > .env
  echo "Created .env with a random FIBER_SECRET_KEY_PASSWORD"
else
  echo ".env already present"
fi

# 3. Config: the image entrypoint only copies its bundled template when
#    /fiber/config.yml is missing, so pre-seeding here wins. Patch the RPC
#    bind so the container port is reachable through the compose mapping.
if [ ! -f fiber-node/config.yml ]; then
  curl -fsSL "$CONFIG_URL" -o fiber-node/config.yml
  sed -i.bak 's/127\.0\.0\.1:8227/0.0.0.0:8227/' fiber-node/config.yml
  rm -f fiber-node/config.yml.bak
  echo "Seeded fiber-node/config.yml (testnet ${FNN_TAG}, RPC bind patched)"
else
  echo "Config already present: fiber-node/config.yml"
fi

# 4. Derive the funding address BEFORE first start — FNN encrypts the key
#    file in place on startup, after which ckb-cli can no longer read it.
KEY_INFO=""
if command -v ckb-cli >/dev/null 2>&1; then
  KEY_INFO="$(ckb-cli util key-info --privkey-path fiber-node/ckb/key 2>/dev/null || true)"
fi

# 5. Bring the node up.
docker compose up -d
echo
echo "Node starting. Follow logs with:  docker compose logs -f fnn"
echo
if [ -n "$KEY_INFO" ]; then
  echo "Fund the testnet address below at https://faucet.nervos.org :"
  echo "$KEY_INFO"
else
  echo "ckb-cli not found (or key already encrypted), so no funding address printed."
  echo "Get it by checking the node logs for the default funding lock script, or by"
  echo "re-creating the key with ckb-cli available before first start."
fi
