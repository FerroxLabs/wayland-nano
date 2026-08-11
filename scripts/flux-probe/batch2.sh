#!/usr/bin/env bash
# B-FLX-02 — Flux fixture batch 2: streaming, tool calls, thinking/cache pass-through.
# Usage: FLUX_TEST_KEY=$(cat ../../.secrets/flux-test-key) ./batch2.sh
# Records bodies only — NEVER auth headers. Safe to commit output.
set -u
KEY="${FLUX_TEST_KEY:?FLUX_TEST_KEY env var required (read from .secrets at call time)}"
BASE="https://api.fluxrouter.ai"
OUT="../../../shared/fixtures/flux"
TS=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$OUT/streaming" "$OUT/tool-calls" "$OUT/thinking" "$OUT/cache" "$OUT/omit-max-tokens" "$OUT/mcp"

probe() { # name url payload_file outfile [extra_headers...]
  local name="$1" url="$2" payload="$3" out="$4"; shift 4
  printf '== %s ==\n' "$name"
  cp "$payload" "${out%.json}_request.json" 2>/dev/null || true
  curl -sS -N -o "$out" -w "HTTP %{http_code} | %{size_download}B | %{time_total}s\n" \
    -H "Authorization: Bearer $KEY" -H "x-api-key: $KEY" \
    -H "anthropic-version: 2023-06-01" -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" "$@" \
    -d @"$payload" "$url"
}

# --- 1. SSE streaming: chat completions ---
cat > /tmp/s-cc.json <<'EOF'
{"model":"flux-fast","max_tokens":512,"stream":true,"messages":[{"role":"user","content":"Count from 1 to 3, one number per line."}]}
EOF
probe "stream chat-completions" "$BASE/v1/chat/completions" /tmp/s-cc.json "$OUT/streaming/${TS}_cc_sse.txt"

# --- 2. SSE streaming: anthropic messages ---
cat > /tmp/s-am.json <<'EOF'
{"model":"flux-auto","max_tokens":512,"stream":true,"messages":[{"role":"user","content":"Count from 1 to 3, one number per line."}]}
EOF
probe "stream anthropic-messages" "$BASE/anthropic/v1/messages" /tmp/s-am.json "$OUT/streaming/${TS}_am_sse.txt"

# --- 3. SSE streaming: responses ---
cat > /tmp/s-rs.json <<'EOF'
{"model":"flux-fast","max_output_tokens":512,"stream":true,"input":"Count from 1 to 3, one number per line."}
EOF
probe "stream responses" "$BASE/v1/responses" /tmp/s-rs.json "$OUT/streaming/${TS}_rs_sse.txt"

# --- 4. Tool call (single function) on completions ---
cat > /tmp/t-cc.json <<'EOF'
{"model":"flux-fast","max_tokens":1024,"stream":false,
 "messages":[{"role":"user","content":"What is the weather in Paris? Use the tool."}],
 "tools":[{"type":"function","function":{"name":"get_weather","description":"Get weather for a city","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}],
 "tool_choice":"auto"}
EOF
probe "tool-call completions" "$BASE/v1/chat/completions" /tmp/t-cc.json "$OUT/tool-calls/${TS}_cc_tool.json"

# --- 5. Tool use on anthropic messages ---
cat > /tmp/t-am.json <<'EOF'
{"model":"flux-auto","max_tokens":1024,
 "messages":[{"role":"user","content":"What is the weather in Paris? Use the tool."}],
 "tools":[{"name":"get_weather","description":"Get weather for a city","input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}]}
EOF
probe "tool-use anthropic" "$BASE/anthropic/v1/messages" /tmp/t-am.json "$OUT/tool-calls/${TS}_am_tool.json"

# --- 6. Thinking pass-through on /anthropic ---
cat > /tmp/th-am.json <<'EOF'
{"model":"flux-auto","max_tokens":2048,"thinking":{"type":"enabled","budget_tokens":1024},
 "messages":[{"role":"user","content":"What is 17*23? Think briefly."}]}
EOF
probe "thinking anthropic" "$BASE/anthropic/v1/messages" /tmp/th-am.json "$OUT/thinking/${TS}_am_thinking.json"

# --- 7. cache_control pass-through on /anthropic (two calls, same cached prefix) ---
LONG=$(printf 'padding sentence %.0s' $(seq 1 300))
cat > /tmp/c-am.json <<EOF
{"model":"flux-auto","max_tokens":64,
 "system":[{"type":"text","text":"You are concise. $LONG","cache_control":{"type":"ephemeral"}}],
 "messages":[{"role":"user","content":"Say ok"}]}
EOF
probe "cache write anthropic" "$BASE/anthropic/v1/messages" /tmp/c-am.json "$OUT/cache/${TS}_am_cache_write.json"
probe "cache read anthropic" "$BASE/anthropic/v1/messages" /tmp/c-am.json "$OUT/cache/${TS}_am_cache_read.json"

# --- 8. Omit max_tokens on completions (#456/#462 contract) ---
cat > /tmp/o-cc.json <<'EOF'
{"model":"flux-fast","stream":false,"messages":[{"role":"user","content":"Say ok"}]}
EOF
probe "omit-max-tokens completions" "$BASE/v1/chat/completions" /tmp/o-cc.json "$OUT/omit-max-tokens/${TS}_cc_omit.json"

# --- 9. MCP tools/list over streamable HTTP ---
cat > /tmp/mcp-init.json <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"wayland-nano-probe","version":"0.1.0"}}}
EOF
printf '== mcp initialize (capture session) ==\n'
curl -sS -N -D "$OUT/mcp/${TS}_init_headers.txt" -o "$OUT/mcp/${TS}_init_body.txt" \
  -w "HTTP %{http_code}\n" \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d @/tmp/mcp-init.json "$BASE/mcp/"
SID=$(grep -i '^mcp-session-id:' "$OUT/mcp/${TS}_init_headers.txt" | tr -d '\r' | awk '{print $2}')
printf 'session-id: %s\n' "${SID:-<none>}"
cat > /tmp/mcp-tl.json <<'EOF'
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
EOF
printf '== mcp tools/list ==\n'
curl -sS -N -o "$OUT/mcp/${TS}_tools_list.txt" -w "HTTP %{http_code}\n" \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  ${SID:+-H "Mcp-Session-Id: $SID"} \
  -d @/tmp/mcp-tl.json "$BASE/mcp/"

echo "batch2 done: $TS"
