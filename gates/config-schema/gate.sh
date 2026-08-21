#!/usr/bin/env bash
# Gate: config-schema. The shipped rules CLI is the parser authority.
set -uo pipefail

BIN="${NANO_CLI_BIN:-target/debug/wayland-nano}"
PROBES="${1:-}"
ROOT="${NANO_REPO_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}"
ASSET_ROOT="${NANO_GATE_ROOT:-$ROOT}"
fails=()
fail() { case " ${fails[*]:-} " in *" $1 "*) ;; *) fails+=("$1 $2");; esac; }
malfunction() { echo "gate: 0/6"; exit 2; }

# WP-3 resolves and confines the artifact before spawning the gate. On Windows
# that canonical path carries the native `\\?\` prefix, which Git Bash cannot
# stat directly. Translate only the two native Windows absolute forms; leave
# Unix paths and already-converted MSYS paths byte-for-byte unchanged.
to_bash_path() {
  local value="$1"
  case "$value" in
    '\\?\'[A-Za-z]:\\*)
      command -v cygpath.exe >/dev/null 2>&1 || malfunction
      cygpath.exe -u "${value#'\\?\'}" | tr -d '\r\n' || malfunction
      ;;
    [A-Za-z]:\\*)
      command -v cygpath.exe >/dev/null 2>&1 || malfunction
      cygpath.exe -u "$value" | tr -d '\r\n' || malfunction
      ;;
    *) printf '%s' "$value" ;;
  esac
}

PROBES="$(to_bash_path "$PROBES")" || malfunction

[ -n "$PROBES" ] && [ -d "$PROBES" ] && [ -n "$ROOT" ] && [ -x "$BIN" ] || malfunction
expected="$(node -e "const {loadCard}=require(process.argv[1]);process.stdout.write(loadCard(process.argv[2]).validation.reference)" "$ASSET_ROOT/gates/lib/card.cjs" "$ASSET_ROOT/gates/config-schema/card.md" 2>/dev/null)" || malfunction
actual="sealed:dir-sha256:$(node "$ASSET_ROOT/gates/lib/dirhash.cjs" "$PROBES" 2>/dev/null)" || malfunction
[ "$expected" = "$actual" ] || malfunction

run_rules() {
  local probe="$1" home
  home="$(mktemp -d "${NANO_CF_RUN_ROOT:-${TMPDIR:-/tmp}}/cf-run-XXXXXX")" || malfunction
  cp "$probe" "$home/rules.toml" && chmod 600 "$home/rules.toml" || { rm -rf "$home"; malfunction; }
  if command -v cygpath.exe >/dev/null 2>&1 && command -v icacls.exe >/dev/null 2>&1; then
    local identity native_rules
    command -v whoami.exe >/dev/null 2>&1 || { rm -rf "$home"; malfunction; }
    identity="$(whoami.exe | tr -d '\r\n')" || { rm -rf "$home"; malfunction; }
    native_rules="$(cygpath.exe -w "$home/rules.toml" | tr -d '\r\n')" || { rm -rf "$home"; malfunction; }
    [ -n "$identity" ] || { rm -rf "$home"; malfunction; }
    icacls.exe "$native_rules" /inheritance:r /grant:r "${identity}:(F)" >/dev/null || { rm -rf "$home"; malfunction; }
  fi
  OUT="$(NANO_HOME="$home" "$BIN" rules 2>&1)"; RC=$?
  rm -rf "$home" || malfunction
}

run_rules "$PROBES/valid.toml"
[ "$RC" -eq 0 ] && [ "$(printf '%s\n' "$OUT" | grep -c '^#[0-9]')" -eq "$(grep -c '^\[\[rule\]\]' "$PROBES/valid.toml")" ] || fail CF-01 execution
for p in unknown_top.toml unknown_rule.toml; do run_rules "$PROBES/$p"; [ "$RC" -ne 0 ] || fail CF-02 security; done
for p in type_exact_string.toml type_decision_int.toml type_pattern_string.toml; do run_rules "$PROBES/$p"; [ "$RC" -ne 0 ] || fail CF-03 security; done
run_rules "$PROBES/deny_heavy.toml"
deny_src="$(grep -c 'decision = "deny"' "$PROBES/deny_heavy.toml")"
deny_out="$(printf '%s\n' "$OUT" | grep -c $'^#[0-9]*\tdeny\t')"
[ "$RC" -eq 0 ] && [ "$deny_src" -gt 0 ] && [ "$deny_src" -eq "$deny_out" ] || fail CF-04 relation

# The public table command must reject rule rows that cannot ever be evaluated
# within the shipped command/token budgets. Exact parser anchors below ensure a
# limit change cannot turn this relational check into a stale duplicate parser.
for p in overlong_command.toml too_many_tokens.toml; do run_rules "$PROBES/$p"; [ "$RC" -ne 0 ] || fail CF-05 value; done

want="$(sed -n 's/^const RECORDED_SHA256: &str = "\([0-9a-f]\{64\}\)";$/\1/p' "$ROOT/crates/nano-model/tests/provider_catalog.rs")"
[ "$(printf '%s\n' "$want" | grep -c .)" -eq 1 ] || fail CF-06 structure
got="$(node -e "const f=require('fs'),c=require('crypto');const b=f.readFileSync(process.argv[1]);process.stdout.write(c.createHash('sha256').update(Buffer.from(b.toString('utf8').replace(/\\r/g,''))).digest('hex'))" "$ROOT/crates/nano-model/data/providerCatalog.vendored.json" 2>/dev/null)" || malfunction
[ -n "$want" ] && [ "$want" = "$got" ] || fail CF-06 structure

# The card's parser anchors are strict validation pins. Drift voids validation
# and is routed to the checks whose authority changed.
anchor="$(node -e "const f=require('fs'),c=require('crypto'),m=require(process.argv[1]);for(const [p,w] of Object.entries(m.parser_anchor)){const g=c.createHash('sha256').update(Buffer.from(f.readFileSync(p,'utf8').replace(/\\r/g,''))).digest('hex');if(g!==w)console.log(p)}" "$ASSET_ROOT/gates/fixtures/config-schema/manifest.json" 2>/dev/null)" || malfunction
if printf '%s\n' "$anchor" | grep -q 'execrules.rs'; then fail CF-02 security; fail CF-03 security; fail CF-05 value; fi
if printf '%s\n' "$anchor" | grep -q 'rules_cmds.rs'; then fail CF-04 relation; fi

for item in ${fails[@]+"${fails[@]}"}; do echo "FAIL $item"; done
echo "gate: $((6 - ${#fails[@]}))/6"
[ "${#fails[@]}" -eq 0 ]
