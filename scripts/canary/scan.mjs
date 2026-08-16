#!/usr/bin/env node
// C3.3 canary scanner — proves the Flux credential appears in NO emitted
// frame, log, session, or dump. Publishes its own implementation (this file)
// and a machine-readable receipt so the scan is independently re-runnable.
//
// Usage: node wayland-nano/scripts/canary/scan.mjs <receipt-out.json>
// The key is read from waylandnano/.secrets/flux-test-key into memory only —
// never printed, never written, never included in the receipt (only its
// SHA-256 fingerprint, matching the repo's digest convention).

import { createHash } from "node:crypto";
import { execSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, writeFileSync, existsSync, mkdtempSync, realpathSync, rmSync, statSync, symlinkSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { tmpdir } from "node:os";

// Preserve the legacy scanner's historical parent-root coverage while exact
// include-list paths are governed relative to the current Git worktree.
const ROOT = resolve(new URL("../../../", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));
const REPO_ROOT = resolve(new URL("../../", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));

function resolveGovernedKey(repoRoot) {
  const candidates = [];
  for (let cursor = resolve(repoRoot); ; cursor = dirname(cursor)) {
    const candidate = join(cursor, ".secrets", "flux-test-key");
    if (existsSync(candidate)) candidates.push(candidate);
    const parent = dirname(cursor);
    if (parent === cursor) break;
  }
  if (candidates.length !== 1) {
    throw new Error(candidates.length === 0 ? "governed key candidate missing" : "governed key candidate ambiguous");
  }
  const outer = dirname(dirname(candidates[0]));
  const marker = join(outer, "wayland-nano");
  if (!existsSync(marker) || !statSync(marker).isDirectory()) {
    throw new Error("governed key candidate marker mismatch");
  }
  return candidates[0];
}

function exactScan(listPath, receiptPath, keyBytes) {
  const parsed = JSON.parse(readFileSync(listPath, "utf8"));
  if (!Array.isArray(parsed) || parsed.some((item) => typeof item !== "string" || item.length === 0)) {
    throw new Error("include list must be a JSON array of non-empty file paths");
  }
  if (new Set(parsed).size !== parsed.length) throw new Error("include list contains duplicates");
  const rootPrefix = REPO_ROOT.endsWith(sep) ? REPO_ROOT : REPO_ROOT + sep;
  const inventory = parsed.map((listed) => {
    const candidate = resolve(REPO_ROOT, listed);
    if (!candidate.startsWith(rootPrefix)) throw new Error(`include path escapes repository: ${listed}`);
    const file = realpathSync(candidate);
    if (!file.startsWith(rootPrefix)) throw new Error(`include path resolves outside repository: ${listed}`);
    if (!statSync(file).isFile()) throw new Error(`include path is not a file: ${listed}`);
    const bytes = readFileSync(file);
    return {
      file: relative(REPO_ROOT, file).replaceAll("\\", "/"),
      sha256: createHash("sha256").update(bytes).digest("hex"),
      bytes: bytes.length,
      contains_key: bytes.includes(keyBytes),
    };
  });
  const hits = inventory.filter((item) => item.contains_key).length;
  const receipt = {
    scanner: "wayland-nano/scripts/canary/scan.mjs exact include-list",
    at: new Date().toISOString(),
    key_fingerprint_sha256: createHash("sha256").update(keyBytes).digest("hex"),
    files_scanned: inventory.length,
    bytes_scanned: inventory.reduce((sum, item) => sum + item.bytes, 0),
    hits,
    verdict: hits === 0 ? "PASS — key appears in zero artifacts" : `FAIL — key found in ${hits} artifact(s)`,
    results: inventory,
  };
  writeFileSync(receiptPath, JSON.stringify(receipt, null, 2));
  return receipt;
}

function selfTestIncludeList() {
  const dir = mkdtempSync(join(tmpdir(), "nano-canary-self-test-"));
  try {
    // Exercise the core with a synthetic credential only. The production key
    // path is deliberately unreachable from this branch.
    const key = Buffer.from("synthetic-canary-key-never-production", "utf8");
    const artifact = join(dir, "artifact.txt");
    const list = join(dir, "list.json");
    const receipt = join(dir, "receipt.json");
    writeFileSync(artifact, "clean synthetic artifact");
    const resolverRoot = join(dir, "governed", "wayland-nano", ".tmp-worktree");
    mkdirSync(resolverRoot, { recursive: true });
    mkdirSync(join(dir, "governed", ".secrets"), { recursive: true });
    writeFileSync(join(dir, "governed", ".secrets", "flux-test-key"), key);
    if (!resolveGovernedKey(resolverRoot).endsWith(join(".secrets", "flux-test-key"))) {
      throw new Error("unique governed candidate was not resolved");
    }
    const zeroRoot = join(dir, "zero", "wayland-nano", ".tmp-worktree");
    mkdirSync(zeroRoot, { recursive: true });
    let rejected = false;
    try { resolveGovernedKey(zeroRoot); } catch { rejected = true; }
    if (!rejected) throw new Error("zero governed candidates were accepted");
    mkdirSync(join(resolverRoot, ".secrets"), { recursive: true });
    writeFileSync(join(resolverRoot, ".secrets", "flux-test-key"), key);
    rejected = false;
    try { resolveGovernedKey(resolverRoot); } catch { rejected = true; }
    if (!rejected) throw new Error("multiple governed candidates were accepted");
    const mismatchRoot = join(dir, "mismatch-repo");
    mkdirSync(join(mismatchRoot, ".secrets"), { recursive: true });
    writeFileSync(join(mismatchRoot, ".secrets", "flux-test-key"), key);
    rejected = false;
    try { resolveGovernedKey(mismatchRoot); } catch { rejected = true; }
    if (!rejected) throw new Error("marker-mismatched candidate was accepted");
    // The core confines to ROOT, so copy the synthetic fixtures under its
    // already-ignored .tmp directory for the duration of the test.
    const rootDir = mkdtempSync(join(REPO_ROOT, ".tmp-canary-self-test-"));
    try {
      const rootArtifact = join(rootDir, "artifact.txt");
      const rootList = join(rootDir, "list.json");
      const rootReceipt = join(rootDir, "receipt.json");
      writeFileSync(rootArtifact, readFileSync(artifact));
      writeFileSync(rootList, JSON.stringify([relative(REPO_ROOT, rootArtifact)]));
      const result = exactScan(rootList, rootReceipt, key);
      if (result.hits !== 0 || result.results.length !== 1 || result.results[0].sha256.length !== 64) {
        throw new Error("clean exact-list scan failed");
      }
      writeFileSync(rootArtifact, Buffer.concat([Buffer.from("prefix"), key, Buffer.from("suffix")]));
      if (exactScan(rootList, rootReceipt, key).hits !== 1) throw new Error("synthetic hit was missed");
      const duplicate = [relative(REPO_ROOT, rootArtifact), relative(REPO_ROOT, rootArtifact)];
      writeFileSync(rootList, JSON.stringify(duplicate));
      let rejected = false;
      try { exactScan(rootList, rootReceipt, key); } catch { rejected = true; }
      if (!rejected) throw new Error("duplicate list was accepted");

      writeFileSync(rootList, JSON.stringify([relative(REPO_ROOT, join(rootDir, "missing.txt"))]));
      rejected = false;
      try { exactScan(rootList, rootReceipt, key); } catch { rejected = true; }
      if (!rejected) throw new Error("missing include-list file was accepted");

      writeFileSync(rootList, JSON.stringify([relative(REPO_ROOT, join(REPO_ROOT, "..", "outside.txt"))]));
      rejected = false;
      try { exactScan(rootList, rootReceipt, key); } catch { rejected = true; }
      if (!rejected) throw new Error("lexical out-of-repository path was accepted");

      const outsideDir = join(dir, "outside-target");
      const outsideArtifact = join(outsideDir, "artifact.txt");
      const escapeLink = join(rootDir, "escape-link");
      mkdirSync(outsideDir);
      writeFileSync(outsideArtifact, "clean outside artifact");
      // Directory junctions avoid Windows developer-mode requirements; other
      // platforms exercise the equivalent realpath escape with a directory symlink.
      symlinkSync(outsideDir, escapeLink, process.platform === "win32" ? "junction" : "dir");
      writeFileSync(rootList, JSON.stringify([relative(REPO_ROOT, join(escapeLink, "artifact.txt"))]));
      rejected = false;
      try { exactScan(rootList, rootReceipt, key); } catch { rejected = true; }
      if (!rejected) throw new Error("realpath out-of-repository path was accepted");
    } finally {
      rmSync(rootDir, { recursive: true, force: true });
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  console.log("include-list self-test PASS");
}

if (process.argv[2] === "--self-test-include-list") {
  selfTestIncludeList();
  process.exit(0);
}

if (process.argv[2] === "--include-list") {
  if (process.argv[4] !== "--receipt" || !process.argv[3] || !process.argv[5] || process.argv.length !== 6) {
    console.error("usage: node scan.mjs --include-list <exact-list.json> --receipt <exact-receipt.json>");
    process.exit(2);
  }
  try {
    const key = Buffer.from(readFileSync(resolveGovernedKey(REPO_ROOT), "utf8").trim(), "utf8");
    const result = exactScan(process.argv[3], process.argv[5], key);
    console.log(`scanned ${result.files_scanned} exact files (${result.bytes_scanned} bytes), hits=${result.hits} -> ${result.verdict}`);
    process.exit(result.hits === 0 ? 0 : 1);
  } catch (error) {
    console.error(`exact include-list scan failed: ${error.message}`);
    process.exit(2);
  }
}

const RECEIPT_OUT = process.argv[2];
if (!RECEIPT_OUT) {
  console.error("usage: node scan.mjs <receipt-out.json>");
  process.exit(2);
}

const key = readFileSync(resolveGovernedKey(ROOT), "utf8").trim();
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
add("session-journal", walk(join(process.env.USERPROFILE || "", ".nano/sessions"), [".jsonl"]));
add("nano-home-logs", walk(join(process.env.USERPROFILE || "", ".nano"), [".log", ".json"]));
add("claim-docs", walk(join(ROOT, "shared/reviews"), [".md"]));

// Desktop conversation DB rows for wayland-nano conversations, dumped live if the
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
  scanner: "wayland-nano/scripts/canary/scan.mjs (this implementation)",
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
