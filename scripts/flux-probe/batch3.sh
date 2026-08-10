#!/usr/bin/env bash
# B-FLX-03 — Flux fixture batch 3: typed errors (402/401), 429/Retry-After,
# mid-stream cancel, x-wl-* response-header inventory.
# Usage: FLUX_TEST_KEY=$(cat <repo-root>/.secrets/flux-test-key) ./batch3.sh
# Records response bodies + RESPONSE headers only — NEVER auth/request headers.
# Review header dumps before commit; safe to commit output.
set -u
KEY="${FLUX_TEST_KEY:?FLUX_TEST_KEY env var required (read from .secrets at call time)}"
BASE="https://api.fluxrouter.ai"
OUT="../../../shared/fixtures/flux"
TS=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$OUT/errors" "$OUT/rate-limit" "$OUT/cancel" "$OUT/headers"

# --- (a1) 402 attempt: over-limit request (max_tokens far above model ceiling) ---
cat > /tmp/b3-402.json <<'EOF'
{"model":"flux-fast","max_tokens":10000000,"stream":false,"messages":[{"role":"user","content":"hi"}]}
EOF
printf '== 402 attempt: over-limit max_tokens ==\n'
cp /tmp/b3-402.json "$OUT/errors/${TS}_cc_overlimit_request.json"
curl -sS -o "$OUT/errors/${TS}_cc_overlimit_response.json" \
  -D "$OUT/errors/${TS}_cc_overlimit_headers.txt" \
  -w "HTTP %{http_code}\n" \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d @/tmp/b3-402.json "$BASE/v1/chat/completions"

# --- (a2) 401 reference: bad key (fallback shape if 402 not reproducible) ---
cat > /tmp/b3-401.json <<'EOF'
{"model":"flux-fast","max_tokens":16,"stream":false,"messages":[{"role":"user","content":"hi"}]}
EOF
printf '== 401 reference: invalid key ==\n'
cp /tmp/b3-401.json "$OUT/errors/${TS}_cc_badkey_request.json"
curl -sS -o "$OUT/errors/${TS}_cc_badkey_response.json" \
  -D "$OUT/errors/${TS}_cc_badkey_headers.txt" \
  -w "HTTP %{http_code}\n" \
  -H "Authorization: Bearer sk-invalid-nanok3-probe-0000000000000000" \
  -H "Content-Type: application/json" \
  -d @/tmp/b3-401.json "$BASE/v1/chat/completions"

# --- (b) 429 burst: rapid sequential requests until rate-limited ---
cat > /tmp/b3-burst.json <<'EOF'
{"model":"flux-fast","max_tokens":1,"stream":false,"messages":[{"role":"user","content":"1"}]}
EOF
printf '== 429 burst ==\n'
cp /tmp/b3-burst.json "$OUT/rate-limit/${TS}_cc_burst_request.json"
: > "$OUT/rate-limit/${TS}_cc_burst_statusline.txt"
for i in $(seq 1 40); do
  code=$(curl -sS -o /tmp/b3-burst-resp.json -D /tmp/b3-burst-hdrs.txt \
    -w "%{http_code}" \
    -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
    -d @/tmp/b3-burst.json "$BASE/v1/chat/completions")
  printf 'req %02d -> HTTP %s\n' "$i" "$code" | tee -a "$OUT/rate-limit/${TS}_cc_burst_statusline.txt"
  if [ "$code" = "429" ]; then
    cp /tmp/b3-burst-resp.json "$OUT/rate-limit/${TS}_cc_429_response.json"
    cp /tmp/b3-burst-hdrs.txt "$OUT/rate-limit/${TS}_cc_429_headers.txt"
    break
  fi
done

# --- (c) mid-stream cancel: abort SSE stream after first ~2KB ---
cat > /tmp/b3-cancel.json <<'EOF'
{"model":"flux-fast","max_tokens":2048,"stream":true,"messages":[{"role":"user","content":"Write a long detailed essay about the history of computing, at least 2000 words."}]}
EOF
printf '== mid-stream cancel ==\n'
cp /tmp/b3-cancel.json "$OUT/cancel/${TS}_cc_stream_request.json"
curl -sS -N --max-time 30 \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -H "Accept: text/event-stream" \
  -d @/tmp/b3-cancel.json "$BASE/v1/chat/completions" 2>"$OUT/cancel/${TS}_cc_stream_cancel_stderr.txt" \
  | head -c 2048 > "$OUT/cancel/${TS}_cc_stream_partial.txt"
printf 'curl pipeline exit (head-close abort): %s\n' "$?" \
  | tee "$OUT/cancel/${TS}_cc_stream_cancel_note.txt"

# --- (d) x-wl-* / non-standard response header inventory on normal completion ---
cat > /tmp/b3-hdr.json <<'EOF'
{"model":"flux-fast","max_tokens":16,"stream":false,"messages":[{"role":"user","content":"Reply with exactly the word: ok"}]}
EOF
printf '== header inventory ==\n'
cp /tmp/b3-hdr.json "$OUT/headers/${TS}_cc_inventory_request.json"
curl -sS -o "$OUT/headers/${TS}_cc_inventory_response.json" \
  -D "$OUT/headers/${TS}_cc_inventory_headers.txt" \
  -w "HTTP %{http_code}\n" \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d @/tmp/b3-hdr.json "$BASE/v1/chat/completions"

printf 'batch3 done (TS=%s)\n' "$TS"
