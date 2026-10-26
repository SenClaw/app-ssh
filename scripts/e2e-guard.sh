#!/usr/bin/env bash
# The command guard end to end: ssh-manager on a scratch HOME, a fake SSH
# server that only records what reached it (examples/fake_sshd.rs), and a fake
# SenClaw answering the model and System One (scripts/fake-senclaw-bridge.py).
# Nothing runs on a real machine and no real model is called.
#
#   cargo build && cargo build --example fake_sshd && scripts/e2e-guard.sh
#
# Needs curl, jq and python3. PORT_APP, PORT_SSH, PORT_BRIDGE and E2E_DIR
# override the defaults; the scratch directory is removed when every check
# passes and kept when one fails.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${BIN:-$REPO/target/debug/ssh-manager}"
SSHD="${SSHD:-$REPO/target/debug/examples/fake_sshd}"
ROOT="${E2E_DIR:-$(mktemp -d)}"
PA="${PORT_APP:-28083}"; PS="${PORT_SSH:-28222}"; PBR="${PORT_BRIDGE:-28788}"
[[ -x "$BIN" && -x "$SSHD" ]] || { echo "build first: cargo build && cargo build --example fake_sshd" >&2; exit 1; }
FAILS=0
SENT="$ROOT/sshd.log"

mkdir -p "$ROOT/home/.senclaw/space-apps-data/ssh-manager" "$ROOT/cwd"
: >"$SENT"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && { kill "$p"; wait "$p"; } 2>/dev/null; done
  if [[ $FAILS -eq 0 ]]; then rm -rf "$ROOT"; else echo "kept $ROOT"; fi
}
trap cleanup EXIT

wait_port() { for _ in $(seq 1 80); do curl -s -o /dev/null "http://127.0.0.1:$1$2" && return 0; sleep 0.1; done; return 1; }

"$SSHD" "$PS" "$SENT" >"$ROOT/sshd.out" 2>&1 & PIDS+=("$!")
python3 -I "$REPO/scripts/fake-senclaw-bridge.py" "$PBR" >"$ROOT/bridge.out" 2>&1 & BRIDGE=$!; PIDS+=("$BRIDGE")
( cd "$ROOT/cwd" && HOME="$ROOT/home" PORT="$PA" SENCLAW_BASE_URL="http://127.0.0.1:$PBR" \
    SENCLAW_SPACE_APP_ID=ssh-manager exec "$BIN" >"$ROOT/app.log" 2>&1 ) & PIDS+=("$!")
wait_port "$PBR" /calls || { echo "bridge did not start"; exit 1; }
wait_port "$PA" /api/hosts || { echo "app did not start"; cat "$ROOT/app.log"; exit 1; }
for _ in $(seq 1 50); do nc -z 127.0.0.1 "$PS" 2>/dev/null && break; sleep 0.1; done

check() { # description, command...
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "  ok   $d"; else echo "  FAIL $d"; FAILS=$((FAILS+1)); fi
}
api() { curl -s -X "$1" "http://127.0.0.1:$PA$2" -H 'content-type: application/json' ${3:+-d "$3"}; }
# tool <name> <arguments json>  → the MCP result object
tool() {
  api POST /api/mcp/message "$(jq -nc --arg n "$1" --argjson a "$2" '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:$n,arguments:$a}}')" | jq -c '.result'
}
text() { jq -r '.content[0].text'; }
HOSTARGS="{\"host\":\"127.0.0.1\",\"port\":$PS,\"user\":\"tester\",\"password\":\"pw\"}"
exec_on() { tool ssh_execute_command "$(jq -c --arg c "$1" '. + {command:$c}' <<<"$2")"; }
reached() { grep -qxF -- "$1" "$SENT"; }
not_reached() { ! grep -qxF -- "$1" "$SENT"; }
calls() { curl -s "http://127.0.0.1:$PBR/calls" | jq -r ".$1"; }
status_of() { tool ssh_command_status "{\"approval_id\":\"$1\"}" | text | jq -r .status; }
wait_status() { # id wanted
  for _ in $(seq 1 50); do [[ "$(status_of "$1")" == "$2" ]] && return 0; sleep 0.1; done; return 1
}
id_of() { sed -n 's/^approval_id: //p'; }

echo "default policy"
check "a fresh install is in smart mode" test "$(api GET /api/settings | jq -r .ssh_command_policy)" = smart

echo "read-only runs at once, no model asked"
R=$(exec_on "df -h" "$HOSTARGS")
check "output comes back" test "$(text <<<"$R")" = "ran: df -h"
check "it reached the server" reached "df -h"
check "no model call for it" test "$(calls llm)" = 0

echo "risky waits for the admin"
R=$(exec_on "ls; rm -rf /srv/data" "$HOSTARGS")
check "reply says not run" grep -q "NOT RUN YET" <<<"$(text <<<"$R")"
check "reply is not an error" test "$(jq -r '.isError // false' <<<"$R")" = false
A1=$(text <<<"$R" | id_of)
check "it has an approval id" test -n "$A1"
check "it did not reach the server" not_reached "ls; rm -rf /srv/data"
check "status is pending" test "$(status_of "$A1")" = pending
LIST=$(api GET /api/guard/approvals)
check "the approval list holds it" test "$(jq -r --arg i "$A1" '.[] | select(.id==$i) | .status' <<<"$LIST")" = pending
check "the password never reaches the UI" bash -c '! grep -q "\"pw\"" <<<"$1"' _ "$LIST"
check "the rules said why" grep -q "rm" <<<"$(jq -r --arg i "$A1" '.[] | select(.id==$i) | .checks[].detail' <<<"$LIST")"

echo "approve runs it where it was meant to run"
check "approve answers running" test "$(api POST "/api/guard/approvals/$A1/approve" '{"remember":false}' | jq -r .status)" = running
check "it finishes" wait_status "$A1" done
check "now it reached the server" reached "ls; rm -rf /srv/data"
check "the agent reads the output" test "$(tool ssh_command_status "{\"approval_id\":\"$A1\"}" | text | jq -r .output)" = "ran: ls; rm -rf /srv/data"
check "a second approve is refused" test "$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PA/api/guard/approvals/$A1/approve")" = 409

echo "reject keeps it off the server"
A2=$(exec_on "reboot" "$HOSTARGS" | text | id_of)
check "reject" test "$(api POST "/api/guard/approvals/$A2/reject" '{"note":"not now"}' | jq -r .status)" = rejected
check "status is rejected with the note" test "$(tool ssh_command_status "{\"approval_id\":\"$A2\"}" | text | jq -r .note)" = "not now"
check "reboot never reached the server" not_reached "reboot"

echo "unknown command cleared by both reviewers is learned"
R=$(exec_on "mytool --status" "$HOSTARGS")
check "it ran" test "$(text <<<"$R")" = "ran: mytool --status"
check "one model call" test "$(calls llm)" = 1
check "System One was asked" test "$(calls decision)" = 1
check "it is in the learned list" test "$(api GET /api/guard/learned | jq -r '.[] | select(.command=="mytool --status") | .source')" = ai
exec_on "mytool    --status" "$HOSTARGS" >/dev/null
check "the second time no model is asked" test "$(calls llm)" = 1

echo "unknown command the model flags waits; approve & remember"
A3=$(exec_on "mytool --apply" "$HOSTARGS" | text | id_of)
check "it waits" test "$(status_of "$A3")" = pending
check "the AI review is on the card" grep -q "high risk" <<<"$(api GET /api/guard/approvals | jq -r --arg i "$A3" '.[] | select(.id==$i) | .checks[].detail')"
api POST "/api/guard/approvals/$A3/approve" '{"remember":true}' >/dev/null
check "it finishes" wait_status "$A3" done
check "remembered as admin" test "$(api GET /api/guard/learned | jq -r '.[] | select(.command=="mytool --apply") | .source')" = admin
check "next time it runs at once" test "$(exec_on "mytool --apply" "$HOSTARGS" | text)" = "ran: mytool --apply"

echo "an open connection"
CID=$(tool ssh_start_connect "$HOSTARGS" | text | grep -oE '[0-9a-f-]{36}' | head -1)
check "connected" test -n "$CID"
check "read-only on the connection" test "$(exec_on "uptime" "{\"connection_id\":\"$CID\"}" | text)" = "ran: uptime"
A4=$(exec_on "systemctl restart nginx" "{\"connection_id\":\"$CID\"}" | text | id_of)
check "risky on the connection waits" test "$(status_of "$A4")" = pending
api POST "/api/guard/approvals/$A4/approve" '{}' >/dev/null
check "approved runs on that connection" wait_status "$A4" done
check "a missing connection is an error, not an approval" test "$(exec_on "reboot" '{"connection_id":"nope"}' | jq -r .isError)" = true

echo "dry run"
D=$(api POST /api/guard/check '{"command":"mytool --status-two"}')
check "dry run says it would run" test "$(jq -r .action <<<"$D")" = run
check "dry run learns nothing" test -z "$(api GET /api/guard/learned | jq -r '.[] | select(.command=="mytool --status-two") | .command')"
check "dry run of a risky one says ask" test "$(api POST /api/guard/check '{"command":"rm -rf /"}' | jq -r .action)" = ask

echo "the REST route is gated too"
R=$(api POST /api/execute "{\"host\":\"127.0.0.1\",\"port\":$PS,\"user\":\"tester\",\"password\":\"pw\",\"command\":\"shutdown -h now\"}")
check "it waits" grep -q "waiting for the admin" <<<"$(jq -r .error <<<"$R")"
check "shutdown never reached the server" not_reached "shutdown -h now"

echo "forget a learned command"
check "delete" test "$(api DELETE /api/guard/learned '{"command":"mytool --status"}' | jq -r .removed)" = true

echo "a reviewer that cannot be reached means the admin decides"
kill "$BRIDGE"; wait "$BRIDGE" 2>/dev/null
R=$(exec_on "othertool --status" "$HOSTARGS")
check "it waits" grep -q "NOT RUN YET" <<<"$(text <<<"$R")"
check "the card says the review could not run" grep -q "could not run" <<<"$(text <<<"$R")"
check "read-only still runs" test "$(exec_on "free -m" "$HOSTARGS" | text)" = "ran: free -m"

echo "allowlist mode checks every command of the line"
S=$(api GET /api/settings | jq -c '.ssh_command_policy="allowlist" | .ssh_allowed_commands=["ls","grep"]')
api PUT /api/settings "$S" >/dev/null
check "ls | grep runs" test "$(exec_on "ls | grep x" "$HOSTARGS" | text)" = "ran: ls | grep x"
check "ls; rm is refused" test "$(exec_on "ls; rm -rf /tmp/x" "$HOSTARGS" | jq -r .isError)" = true
check "rm never reached the server" not_reached "ls; rm -rf /tmp/x"

echo "logs"
check "decisions are logged under guard" test "$(api GET '/api/logs?limit=500' | jq '[.[] | select(.source=="guard")] | length')" -ge 8

echo
if [[ $FAILS -eq 0 ]]; then echo "all checks passed"; else echo "$FAILS check(s) failed"; fi
exit $FAILS
