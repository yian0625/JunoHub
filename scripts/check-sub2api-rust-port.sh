#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT_DIR"

adapter_calls="$(rg -c 'admin_request\(' junohub-server/src/sub2api.rs || true)"
go_image_refs="$(rg -c 'weishaw/sub2api' compose.yaml || true)"
go_gateway_refs="$( (rg -c 'http://sub2api' junohub-server/src junohub-web compose.yaml -g '!junohub-web/.next/**' || true) | awk -F: '{sum += $NF} END {print sum+0}')"
redis_refs="$(rg -c '^redis\s*=' junohub-server/Cargo.toml || true)"
unchecked="$(rg -c '^- \[ \]' junohub-server/SUB2API_RUST_PORT.md || true)"

adapter_calls="${adapter_calls:-0}"
go_image_refs="${go_image_refs:-0}"
go_gateway_refs="${go_gateway_refs:-0}"
redis_refs="${redis_refs:-0}"
unchecked="${unchecked:-0}"

cat <<EOF
Sub2API Rust port status
------------------------
Go adapter call sites : $adapter_calls
Go image references   : $go_image_refs
Go gateway references : $go_gateway_refs
Rust Redis dependency : $redis_refs
Unchecked milestones  : $unchecked
EOF

complete=true
[[ "$adapter_calls" == "0" ]] || complete=false
[[ "$go_image_refs" == "0" ]] || complete=false
[[ "$go_gateway_refs" == "0" ]] || complete=false
[[ "$redis_refs" != "0" ]] || complete=false
[[ "$unchecked" == "0" ]] || complete=false

if [[ "$complete" == "true" ]]; then
  echo "Status                 : COMPLETE"
  exit 0
fi

echo "Status                 : INCOMPLETE"
if [[ "${1:-}" == "--require-complete" ]]; then
  exit 1
fi
