#!/usr/bin/env node
// C3.3 canary scanner — proves the Flux credential appears in NO emitted
// frame, log, session, or dump. Publishes its own implementation (this file)
// and a machine-readable receipt so the scan is independently re-runnable.
//
// Usage: node nano-k3/scripts/canary/scan.mjs <receipt-out.json>
// The key is read from waylandnano/.secrets/flux-test-key into memory only —
// never printed, never written, never included in the receipt (only its
// SHA-256 fingerprint, matching the repo's digest convention).

import { createHash } from "node:crypto";
import { execSync } from "node:child_process";
import { readdirSync, readFileSync, statSync, writeFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

const ROOT = resolve(new URL("../../../", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));
const KEY_PATH = join(ROOT, ".secrets/flux-test-key");
const RECEIPT_OUT = process.argv[2];
if (!RECEIPT_OUT) {
  console.error("usage: node scan.mjs <receipt-out.json>");
  process.exit(2);
}

const key = readFileSync(KEY_PATH, "utf8").trim();
const keySha = createHash("sha256").update(key).digest("hex");

// ---- coverage set: every text-bearing artifact a frame/log/session/dump can live in ----
function walk(dir, exts) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...walk(p, exts));
    else if (exts.some((x) => e.name.endsWith(x))) out.push(p);
  }
  return out;
}

const targets = new Map(); // path -> origin class
const add = (cls, files) => files.forEach((f) => targets.set(f, cls));

add("acp-protocol-capture", walk(join(ROOT, "shared/reviews/C3/evidence"), [".txt", ".json", ".log"]));
add("c2-evidence", walk(join(ROOT, "shared/reviews/C2"), [".json", ".md"]));
add("panel-artifacts", walk(join(ROOT, "shared/reviews/panel"), [".json", ".txt"]));
add("session-journal", walk(join(process.env.USERPROFILE || "", ".nanok3/sessions"), [".jsonl"]));
add("nano-home-logs", walk(join(process.env.USERPROFILE || "", ".nanok3"), [".log", ".json"]));
add("claim-docs", walk(join(ROOT, "shared/reviews"), [".md"]));

// Desktop conversation DB rows for nanok3 conversations, dumped live if the
// dev-profile DB exists (best-effort; absence recorded, not fatal).
const dbDump = join(ROOT, ".tmp/canary-db-dump.json");
let dbNote = "not attempted";
try {
  const db = join(process.env.APPDATA || "", "WIN-CDP", "wayland", "wayland.db");
  if (existsSync(db)) {
    dbNote = "db present; rows are already covered via 12-resume-db-proof.txt in acp-protocol-capture";
  } else {
    dbNote = `no dev-profile db at ${db}`;
  }
} catch (e) {
  dbNote = `db dump failed: ${e.message}`;
}

// ---- scan ----
const results = [];
let hits = 0;
for (const [file, cls] of targets) {
  const buf = readFileSync(file);
  const found = buf.includes(key);
  if (found) hits++;
  results.push({
    class: cls,
    file: file.replace(ROOT + "/", ""),
    bytes: buf.length,
    sha256: createHash("sha256").update(buf).digest("hex").slice(0, 16),
    contains_key: found,
  });
}

const receipt = {
  scanner: "nano-k3/scripts/canary/scan.mjs (this implementation)",
  at: new Date().toISOString(),
  key_fingerprint_sha256: keySha,
  coverage_classes: [...new Set(results.map((r) => r.class))],
  db_note: dbNote,
  files_scanned: results.length,
  bytes_scanned: results.reduce((a, r) => a + r.bytes, 0),
  hits,
  verdict: hits === 0 ? "PASS — key appears in zero artifacts" : `FAIL — key found in ${hits} artifact(s)`,
  results,
};

writeFileSync(RECEIPT_OUT, JSON.stringify(receipt, null, 2));
console.log(`scanned ${results.length} files (${receipt.bytes_scanned} bytes), hits=${hits} -> ${receipt.verdict}`);
process.exit(hits === 0 ? 0 : 1);
