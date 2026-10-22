#!/usr/bin/env bash
# Two ssh-manager instances on scratch HOMEs, synced the way the SenClaw daemon
# will: digest -> read what differs -> apply on the other side. No daemon and no
# real data: HOME, ports and access tokens are all scratch.
#
#   cargo build && scripts/e2e-sync.sh
#
# Needs curl and jq. BIN, PORT_A, PORT_B and E2E_DIR override the defaults; the
# scratch directory is removed when every check passes and kept when one fails.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${BIN:-$REPO/target/debug/ssh-manager}"
ROOT="${E2E_DIR:-$(mktemp -d)}"
PA="${PORT_A:-28081}"; PB="${PORT_B:-28082}"
[[ -x "$BIN" ]] || { echo "build first: $BIN not found" >&2; exit 1; }
TA="sca_$(printf 'a%.0s' {1..64})"; TB="sca_$(printf 'b%.0s' {1..64})"
FAILS=0

mkdir -p "$ROOT/a/home" "$ROOT/b/home" "$ROOT/a/cwd" "$ROOT/b/cwd"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && { kill "$p"; wait "$p"; } 2>/dev/null; done
  if [[ $FAILS -eq 0 ]]; then rm -rf "$ROOT"; else echo "kept $ROOT"; fi
}
trap cleanup EXIT

start() { # name port token
  mkdir -p "$ROOT/$1/home/.senclaw/space-apps-data/ssh-manager"
  # exec, so the pid we record is the app itself and not a subshell around it
  ( cd "$ROOT/$1/cwd" && HOME="$ROOT/$1/home" PORT="$2" SENCLAW_TOKEN_ACCESS_APP="$3" exec "$BIN" >"$ROOT/$1/app.log" 2>&1 ) &
  echo $! >"$ROOT/$1/pid"
  PIDS+=("$!")
  for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$2/api/hosts" && return 0; sleep 0.1; done
  echo "instance $1 did not start"; cat "$ROOT/$1/app.log"; exit 1
}
stop() { local p; p=$(cat "$ROOT/$1/pid"); kill "$p" 2>/dev/null; { wait "$p"; } 2>/dev/null; }

check() { # description, command...
  local d="$1"; shift
  if "$@" >/dev/null 2>&1; then echo "  ok   $d"; else echo "  FAIL $d"; FAILS=$((FAILS+1)); fi
}
eqv() { [[ "$1" == "$2" ]]; }

# api <instance> <method> <path> [json]  (authenticated)
api() {
  local port token; if [[ $1 == a ]]; then port=$PA; token=$TA; else port=$PB; token=$TB; fi
  curl -s -X "$2" "http://127.0.0.1:$port$3" -H "X-SenClaw-App-Token: $token" -H 'content-type: application/json' ${4:+-d "$4"}
}
# open <instance> <method> <path> [json]  (no token, as a browser or another program would)
open_() {
  local port; if [[ $1 == a ]]; then port=$PA; else port=$PB; fi
  curl -s -o /dev/null -w '%{http_code}' -X "$2" "http://127.0.0.1:$port$3" -H 'content-type: application/json' ${4:+-d "$4"}
}

COLLS='["keychain","hosts","port_forwarding","snippets"]'

# Ship from $1 to $2 what $2 lacks or holds older (digests in $3 / $4), as the daemon will.
ship() {
  local from=$1 to=$2 dfrom=$3 dto=$4 flags=$5 recs='[]' c ids rf
  for c in $(jq -r '.[]' <<<"$COLLS"); do
    ids=$(jq -nc --argjson f "$dfrom" --argjson t "$dto" --arg c "$c" '
      ($t.collections[$c] // [] | map({(.id): .hlc}) | add // {}) as $have
      | [ $f.collections[$c][]? | select(($have[.id] // "") < .hlc) | .id ]')
    [[ "$ids" == "[]" ]] && continue
    rf=$(jq -nc --arg c "$c" --argjson ids "$ids" --argjson fl "$flags" '{collection:$c, ids:$ids, secrets:($fl.secrets // false)}')
    recs=$(jq -nc --argjson a "$recs" --argjson b "$(api $from POST /api/sync/read "$rf")" '$a + $b.records')
  done
  api $to POST /api/sync/apply "$(jq -nc --argjson r "$recs" --argjson fl "$flags" '$fl + {records:$r}')" >"$ROOT/last-$to"
}

# One exchange, both directions. $1,$2 = instances; $3 = apply flags as a JSON object.
exchange() {
  local x=$1 y=$2 flags=${3:-'{}'} dx dy
  dx=$(api $x POST /api/sync/state '{}'); dy=$(api $y POST /api/sync/state '{}')
  ship $x $y "$dx" "$dy" "$flags"
  ship $y $x "$dy" "$dx" "$flags"
}

# What the two instances hold, without anything machine-local or secret.
view() { # instance
  local out='{}'
  for p in hosts keychain snippets port-forwarding; do
    out=$(jq -n --argjson o "$out" --arg p "$p" --argjson v "$(api $1 GET /api/$p)" \
      '$o + {($p): ($v | map(del(.password, .value, .active, .enabled, .running, .status, .host_name, .listening_on, .started_at, .uptime_seconds, .connections, .total_connections, .last_error)) | sort_by(.id))}')
  done
  echo "$out"
}

echo "== start two instances (scratch HOME, ports $PA/$PB)"
# B starts over data an older build wrote: 0644, no sync.json
DB="$ROOT/b/home/.senclaw/space-apps-data/ssh-manager"; mkdir -p "$DB"
LEGACY='{"legacy-1":{"id":"legacy-1","name":"legacy box","host":"legacy.example","port":22,"user":"old","password":"legacy-pw","keychain_id":null,"tags":[]}}'
printf '%s' "$LEGACY" >"$DB/hosts.json"; chmod 644 "$DB/hosts.json"
start a $PA "$TA"; start b $PB "$TB"
check "legacy data was backed up untouched" eqv "$(cat "$DB/hosts.json.pre-sync.bak")" "$LEGACY"
check "legacy file tightened to owner-only" eqv "$(stat -f '%Lp' "$DB/hosts.json")" 600
check "legacy host still there, password intact" eqv "$(api b GET /api/hosts | jq -r '.[0].password')" legacy-pw

echo "== the sync routes answer only to the app token"
check "no token -> 401"            eqv "$(open_ a GET /api/sync/schema)" 401
check "wrong token -> 401"         eqv "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$PA/api/sync/schema -H "X-SenClaw-App-Token: $TB")" 401
check "right token -> 200"         eqv "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:$PA/api/sync/schema -H "X-SenClaw-App-Token: $TA")" 200
check "state without token -> 401" eqv "$(open_ a POST /api/sync/state '{}')" 401
check "apply without token -> 401" eqv "$(open_ a POST /api/sync/apply '{"records":[]}')" 401
check "the rest of the app is unchanged (hosts list open)" eqv "$(open_ a GET /api/hosts)" 200
SCHEMA=$(api a GET /api/sync/schema)
check "schema names 4 collections in order" eqv "$(jq -c .order <<<"$SCHEMA")" '["keychain","hosts","port_forwarding","snippets"]'
check "the two instances have different device ids" test "$(jq -r .deviceId <<<"$SCHEMA")" != "$(api b GET /api/sync/schema | jq -r .deviceId)"

echo "== seed A and B through the app's own REST API"
HA1=$(api a POST /api/hosts '{"id":"","name":"web","host":"web.example","port":22,"user":"root","password":"a-web-pw","keychain_id":null,"tags":["prod"]}' | jq -r .id)
HA2=$(api a POST /api/hosts '{"id":"","name":"db","host":"db.example","port":22,"user":"admin","password":null,"keychain_id":null,"tags":[]}' | jq -r .id)
KA=$(api a POST /api/keychain '{"id":"","name":"deploy key","item_type":"PrivateKey","value":"-----a-private-key-----"}' | jq -r .id)
api a POST /api/snippets '{"id":"","name":"uptime","script":"uptime"}' >/dev/null
RA=$(api a POST /api/port-forwarding "{\"id\":\"\",\"name\":\"db tunnel\",\"local_port\":15432,\"bind_address\":\"127.0.0.1\",\"host_id\":\"$HA2\",\"destination_address\":\"localhost\",\"destination_port\":5432,\"active\":false}" | jq -r .id)
RW=$(api a POST /api/port-forwarding "{\"id\":\"\",\"name\":\"web tunnel\",\"local_port\":18080,\"bind_address\":\"127.0.0.1\",\"host_id\":\"$HA1\",\"destination_address\":\"localhost\",\"destination_port\":80,\"active\":false}" | jq -r .id)
# B already has its own copy of "web" (different id), and one host A lacks
HB1=$(api b POST /api/hosts '{"id":"","name":"web (b)","host":"web.example","port":22,"user":"root","password":"b-web-pw","keychain_id":null,"tags":[]}' | jq -r .id)
api b POST /api/hosts '{"id":"","name":"build","host":"build.example","port":2222,"user":"ci","password":null,"keychain_id":null,"tags":[]}' >/dev/null
api b POST /api/snippets '{"id":"","name":"df","script":"df -h"}' >/dev/null

echo "== a dry run changes nothing and says what would happen"
before=$(view b)
DRY=$(api a POST /api/sync/read "{\"collection\":\"hosts\",\"ids\":[\"$HA1\",\"$HA2\"]}" | jq -c '{records: .records, dryRun: true}')
check "dry run reports a duplicate for the host B already has" \
  eqv "$(api b POST /api/sync/apply "$DRY" | jq -r '[.results[]|select(.id=="'$HA1'")|.status]|first')" duplicate
check "dry run left B untouched" eqv "$(view b)" "$before"

echo "== first exchange, not asked to merge duplicates"
exchange a b; exchange a b
check "web is reported as a duplicate, not added twice" eqv "$(api b GET /api/hosts | jq '[.[]|select(.host=="web.example")]|length')" 1
check "the tunnel to the unrelated host arrived" eqv "$(api b GET /api/port-forwarding | jq -r '[.[]|.id]|sort|join(",")')" "$RA"
check "the tunnel to the duplicate host waits with it" eqv "$(jq -r '[.results[]|select(.id=="'$RW'")|.reason]|first' "$ROOT/last-b")" "waiting_for:host_id"
check "A's other host arrived on B" test "$(api b GET /api/hosts | jq -r '[.[]|.name]|index("db")')" != null
check "no password crossed: A's db/web hosts on B have none from A" eqv "$(api b GET /api/hosts | jq -r '[.[]|select(.name=="db")|.password]|first')" null
check "B's own web password is still B's" eqv "$(api b GET /api/hosts | jq -r '[.[]|select(.id=="'$HB1'")|.password]|first')" b-web-pw
check "the keychain item arrived as a placeholder without its key" eqv "$(api b GET /api/keychain | jq -r '[.[]|select(.id=="'$KA'")|.value]|first')" ""

echo "== exchange again, merging duplicates"
for _ in 1 2 3; do exchange a b '{"adoptMatches":true}'; done
check "one web host on each side" eqv "$(api a GET /api/hosts | jq '[.[]|select(.host=="web.example")]|length')/$(api b GET /api/hosts | jq '[.[]|select(.host=="web.example")]|length')" 1/1
check "A and B hold the same records" eqv "$(view a)" "$(view b)"
WEB=$(api a GET /api/hosts | jq -r '[.[]|select(.host=="web.example")|.id]|first')
check "both settled on the smaller id" eqv "$WEB" "$(printf '%s\n%s\n' "$HA1" "$HB1" | sort | head -1)"
check "B's db tunnel still points at its host" eqv "$(api b GET /api/port-forwarding | jq -r '[.[]|select(.id=="'$RA'")|.host_id]|first')" "$HA2"
check "B's web tunnel points at the surviving web host" eqv "$(api b GET /api/port-forwarding | jq -r '[.[]|select(.id=="'$RW'")|.host_id]|first')" "$WEB"
check "tunnels arrived switched off" eqv "$(api b GET /api/port-forwarding | jq -r '[.[]|.enabled]|any')" false
check "the two devices agree on every stamp" eqv "$(api a POST /api/sync/state '{}' | jq -c '.collections')" "$(api b POST /api/sync/state '{}' | jq -c '.collections')"

echo "== edit on A, delete on B, exchange"
api a PUT /api/snippets/$(api a GET /api/snippets | jq -r '[.[]|select(.name=="uptime")|.id]|first') '{"id":"","name":"uptime","script":"uptime -p"}' >/dev/null
DEL=$(api a GET /api/hosts | jq -r '[.[]|select(.name=="db")|.id]|first')
api b DELETE /api/hosts/$DEL >/dev/null
sleep 0.05
exchange a b; exchange a b
check "the snippet edit reached B" eqv "$(api b GET /api/snippets | jq -r '[.[]|select(.name=="uptime")|.script]|first')" "uptime -p"
check "the host deleted on B is gone from A" eqv "$(api a GET /api/hosts | jq '[.[]|select(.id=="'$DEL'")]|length')" 0
check "A and B hold the same records" eqv "$(view a)" "$(view b)"

echo "== a change that would redirect a stored password is held"
api b PUT /api/hosts/$WEB "$(api b GET /api/hosts | jq -c '[.[]|select(.id=="'$WEB'")|.+{password:"b-pw"}]|first')" >/dev/null
api a PUT /api/hosts/$WEB "$(api a GET /api/hosts | jq -c '[.[]|select(.id=="'$WEB'")|.+{host:"evil.example"}]|first')" >/dev/null
exchange a b
check "B kept pointing at the old address" eqv "$(api b GET /api/hosts | jq -r '[.[]|select(.id=="'$WEB'")|.host]|first')" web.example
check "the apply result says why" eqv "$(jq -r '[.results[]|select(.id=="'$WEB'")|.reason]|first' "$ROOT/last-b")" credential_target_changed
exchange a b '{"allowRisky":true}'
check "once allowed it goes through" eqv "$(api b GET /api/hosts | jq -r '[.[]|select(.id=="'$WEB'")|.host]|first')" evil.example
check "and B's password is still its own" eqv "$(api b GET /api/hosts | jq -r '[.[]|select(.id=="'$WEB'")|.password]|first')" b-pw

echo "== secrets only travel when both ends opt in"
# The stamps already agree, so nothing is "newer"; read the record again, now with its secret.
KREC=$(api a POST /api/sync/read "{\"collection\":\"keychain\",\"ids\":[\"$KA\"],\"secrets\":true}" | jq -c .records)
check "A hands out the key only when asked" eqv "$(jq -r '.[0].data.value' <<<"$KREC")" "-----a-private-key-----"
check "B ignores it while it does not accept secrets" eqv "$(api b POST /api/sync/apply "{\"records\":$KREC}" >/dev/null; api b GET /api/keychain | jq -r '[.[]|select(.id=="'$KA'")|.value]|first')" ""
api b POST /api/sync/apply "{\"records\":$KREC,\"secrets\":true}" >/dev/null
check "now the key crossed" eqv "$(api b GET /api/keychain | jq -r '[.[]|select(.id=="'$KA'")|.value]|first')" "-----a-private-key-----"

echo "== a restart keeps the stamps and does not invent edits"
st_before=$(api b POST /api/sync/state '{}' | jq -c .collections)
stop b; start b $PB "$TB"
check "same stamps after restart" eqv "$(api b POST /api/sync/state '{}' | jq -c .collections)" "$st_before"
check "same device id after restart" eqv "$(api b GET /api/sync/schema | jq -r .deviceId)" "$(jq -r .device_id "$ROOT/b/home/.senclaw/space-apps-data/ssh-manager/sync.json")"

echo "== files on disk"
D="$ROOT/b/home/.senclaw/space-apps-data/ssh-manager"
check "data files are owner-only" eqv "$(stat -f '%Lp' "$D"/*.json | sort -u | tr '\n' ' ')" "600 "
check "the folder is owner-only" eqv "$(stat -f '%Lp' "$D")" 700
check "pre-sync backup still there" test -f "$D/hosts.json.pre-sync.bak"
check "hosts.json still parses as the old shape (id -> record)" eqv "$(jq -r 'type' "$D/hosts.json")" object
check "no tombstones inside the collection files" eqv "$(jq -r '[.[]|select(.name==null)]|length' "$D/hosts.json")" 0
check "the log holds counts, not hosts" eqv "$(api b GET /api/logs | jq '[.[]|select(.kind=="apply")|.message]|map(test("example|pw"))|any')" false

echo
if [[ $FAILS -eq 0 ]]; then echo "ALL PASSED"; else echo "$FAILS FAILED"; exit 1; fi
