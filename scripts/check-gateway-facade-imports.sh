#!/usr/bin/env bash
# Phase A layering check: gateway must import common only via ::facade (not fsm/twin_runtime/…).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
violations="$(rg 'common::(fsm|engine|twin_runtime|digital_twin|published|vehicle_physics|vehicle_state|vehicle_constants|vehicle_kinematics|diagnostic|transition_sink)::' \
  "$root/crates/gateway" --glob '*.rs' || true)"
if [[ -n "$violations" ]]; then
  echo "gateway must use common::facade only; direct internal common imports found:"
  echo "$violations"
  exit 1
fi
echo "ok: gateway imports only common::facade (no internal common module paths)"
