#!/usr/bin/env python3
"""F-6 live proof: a cron job created THROUGH the `cronjob` tool (model tool
call -> ACP permission prompt -> journal-first create) fires correctly after
the creating host is KILLED (kill-resume through a fresh acp-host).

Flow:
  1. host A (acp-host, isolated NANO_HOME): initialize -> session/new ->
     session/prompt instructing the model to call `cronjob` create with
     schedule "* * * * *". The driver answers the session/request_permission
     prompt with "allow" (proving the create-prompts-even-outside-read_only
     gate arm is live on the interactive surface).
  2. Assert the job exists in <NANO_HOME>/cron/jobs.json AND a cron_created
     op is journaled in the session journal (the production creation path,
     no externally-authored jobs.json).
  3. Kill host A BEFORE the first fire minute elapses.
  4. host B (fresh acp-host, same NANO_HOME): poll the session journal until
     a cron_fired op + a completed provenance-marked cron turn appear
     (<= ~150s: one 30s tick + one minute boundary + one minimal model turn).
  5. Assert exactly ONE cron_fired for the job (no double fire across the
     restart), the fired turn input carries the provenance prefix, and the
     turn completed.

Credential discipline: the Flux key is read from the path named by
FLUX_TEST_KEY_FILE (default ../.secrets/flux-test-key relative to the repo
root) into the child ENVIRONMENT only. It is never written to any artifact;
every captured log is canary-scanned for the key value at the end (0 hits
required).

Usage: python f6_cron_create_proof.py <repo_root> <work_dir>
Exit 0 = PASS, 2 = FAIL (reasons on stdout), 3 = self-skip (no key).
"""

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

TICK_SECS = 30
FIRE_DEADLINE_SECS = 200


class Host:
    """One acp-host process speaking NDJSON JSON-RPC over stdio."""

    def __init__(self, binary: Path, env: dict, workspace: Path, log_path: Path):
        self.log = open(log_path, "w", encoding="utf-8")
        self.proc = subprocess.Popen(
            [str(binary), "acp-host"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT if False else subprocess.PIPE,
            cwd=str(workspace),
            env=env,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self.pending = {}
        self.responses = {}
        self.requests_from_host = []
        self.notifications = []
        self._lock = threading.Lock()
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()
        self._stderr = threading.Thread(target=self._stderr_loop, daemon=True)
        self._stderr.start()

    def _read_loop(self):
        for line in self.proc.stdout:
            self.log.write(line)
            self.log.flush()
            try:
                frame = json.loads(line)
            except json.JSONDecodeError:
                continue
            with self._lock:
                if "method" in frame and "id" in frame:
                    self.requests_from_host.append(frame)
                elif "method" in frame:
                    self.notifications.append(frame)
                elif "id" in frame:
                    self.responses[frame["id"]] = frame

    def _stderr_loop(self):
        for line in self.proc.stderr:
            self.log.write("[stderr] " + line)
            self.log.flush()

    def call(self, method: str, params: dict, timeout: float = 120.0) -> dict:
        rid = self.next_id
        self.next_id += 1
        frame = {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(frame) + "\n")
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self._lock:
                if rid in self.responses:
                    return self.responses.pop(rid)
            self._answer_permission_requests()
            time.sleep(0.05)
        raise TimeoutError(f"no response to {method} within {timeout}s")

    def _answer_permission_requests(self):
        with self._lock:
            pending = [f for f in self.requests_from_host]
        for frame in pending:
            if frame.get("method") != "session/request_permission":
                continue
            answer = {
                "jsonrpc": "2.0",
                "id": frame["id"],
                "result": {"outcome": {"outcome": "selected", "optionId": "allow"}},
            }
            self.proc.stdin.write(json.dumps(answer) + "\n")
            self.proc.stdin.flush()
            with self._lock:
                self.requests_from_host.remove(frame)
            print(f"  -> answered permission prompt id={frame['id']} with allow")

    def kill(self):
        try:
            self.proc.kill()
            self.proc.wait(timeout=10)
        except Exception:
            pass
        self.log.close()


def journal_ops(journal_path: Path) -> list:
    ops = []
    for line in journal_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            ops.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return ops


def main() -> int:
    repo = Path(sys.argv[1]).resolve()
    work = Path(sys.argv[2]).resolve()
    key_file = Path(os.environ["FLUX_TEST_KEY_FILE"]) if "FLUX_TEST_KEY_FILE" in os.environ else None
    if key_file is None:
        # The key lives at <workspace-root>/.secrets/flux-test-key — the
        # repo's parent normally, the worktree's grandparent from .tmp-wt-*.
        for candidate in [repo.parent / ".secrets" / "flux-test-key",
                          repo.parent.parent / ".secrets" / "flux-test-key"]:
            if candidate.exists():
                key_file = candidate
                break
    if key_file is None or not key_file.exists():
        print("SELF-SKIP: no Flux test key found (set FLUX_TEST_KEY_FILE)")
        return 3
    key = key_file.read_text(encoding="utf-8").strip()

    binary = repo / "target" / "debug" / ("wayland-nano.exe" if os.name == "nt" else "wayland-nano")
    if not binary.exists():
        print(f"FAIL: binary missing: {binary} (run cargo build -p nano-cli first)")
        return 2

    work.mkdir(parents=True, exist_ok=True)
    home = work / "nano_home"
    workspace = work / "workspace"
    workspace.mkdir(exist_ok=True)

    env = dict(os.environ)
    env["NANO_HOME"] = str(home)
    env["FLUX_API_KEY"] = key  # env-at-spawn only; never written to artifacts

    # ── Phase 1: host A creates the job THROUGH the cronjob tool ──
    print("phase 1: host A — model-driven cronjob create through the ACP gate")
    host_a = Host(binary, env, workspace, work / "host-a.log")
    init = host_a.call("initialize", {"protocolVersion": 1})
    if "error" in init:
        print(f"FAIL: initialize: {init['error']}")
        return 2
    new = host_a.call("session/new", {"cwd": str(workspace), "mcpServers": []})
    session_id = new.get("result", {}).get("sessionId")
    if not session_id:
        print(f"FAIL: session/new: {new}")
        return 2
    print(f"  session: {session_id}")
    prompt = (
        "Call the cronjob tool exactly once with these exact arguments: "
        'action="create", schedule="* * * * *", prompt="Reply with exactly: cron-fired-ok". '
        "Do not call any other tool. After the tool returns, reply with the job id only."
    )
    resp = host_a.call(
        "session/prompt",
        {"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]},
        timeout=180.0,
    )
    if "error" in resp:
        print(f"FAIL: session/prompt: {resp['error']}")
        host_a.kill()
        return 2
    stop = resp.get("result", {}).get("stopReason")
    print(f"  create turn stopReason: {stop}")

    jobs_path = home / "cron" / "jobs.json"
    if not jobs_path.exists():
        print("FAIL: jobs.json not created — the model did not create the job through the tool")
        host_a.kill()
        return 2
    jobs = json.loads(jobs_path.read_text(encoding="utf-8"))
    our = [j for j in jobs if "cron-fired-ok" in j.get("prompt", "")]
    if len(our) != 1:
        print(f"FAIL: expected exactly 1 proof job in jobs.json, got {len(our)}: {jobs}")
        host_a.kill()
        return 2
    job = our[0]
    job_id = job["job_id"]
    print(f"  jobs.json carries job {job_id} schedule={job['schedule']!r}")

    journal_path = home / "sessions" / f"{session_id}.jsonl"
    ops = journal_ops(journal_path)
    created = [
        e for e in ops
        if e.get("op", {}).get("type") == "cron_created" and e["op"].get("job_id") == job_id
    ]
    if len(created) != 1:
        print(f"FAIL: expected 1 cron_created op for {job_id}, got {len(created)}")
        host_a.kill()
        return 2
    print("  cron_created journaled (journal-first create through the tool)")

    # The create MUST have taken the gate prompt (the locked §5.5 ruling —
    # scheduled code execution is never auto-approved, on ANY mode).
    log_a = (work / "host-a.log").read_text(encoding="utf-8", errors="replace")
    perm_frames = [
        json.loads(line) for line in log_a.splitlines()
        if line.strip().startswith("{") and '"session/request_permission"' in line
    ]
    cron_prompts = [
        f for f in perm_frames
        if f.get("params", {}).get("toolCall", {}).get("title") == "cronjob"
    ]
    if not cron_prompts:
        print("FAIL: no session/request_permission frame for the cronjob create — "
              "the always-prompt gate arm did not fire")
        host_a.kill()
        return 2
    print(f"  gate prompt proven live: {len(cron_prompts)} cronjob permission frame(s)")

    # ── Phase 2: kill host A before the fire; host B resumes and fires ──
    print("phase 2: killing host A before the first fire")
    host_a.kill()
    time.sleep(1)

    print("phase 3: host B (fresh process, same NANO_HOME) — kill-resume fire")
    host_b = Host(binary, env, workspace, work / "host-b.log")
    host_b.call("initialize", {"protocolVersion": 1})
    deadline = time.time() + FIRE_DEADLINE_SECS
    fired = None
    while time.time() < deadline:
        ops = journal_ops(journal_path)
        fired = [
            e for e in ops
            if e.get("op", {}).get("type") == "cron_fired" and e["op"].get("job_id") == job_id
        ]
        if fired:
            break
        time.sleep(2)
    if not fired:
        print(f"FAIL: no cron_fired for {job_id} within {FIRE_DEADLINE_SECS}s")
        host_b.kill()
        return 2
    if len(fired) != 1:
        print(f"FAIL: cron_fired count for {job_id} = {len(fired)} (double fire!)")
        host_b.kill()
        return 2
    fire_op = fired[0]["op"]
    print(f"  cron_fired journaled: occurrence {fire_op['occurrence_id']} "
          f"mode_at_fire={fire_op['mode_at_fire']} coalesced={fire_op.get('coalesced', 0)}")

    # Wait for the fired turn to complete (one minimal model turn).
    turn_id = fire_op["turn_id"]
    done_deadline = time.time() + 180
    turn_done = False
    while time.time() < done_deadline:
        ops = journal_ops(journal_path)
        begun = [
            e for e in ops
            if e.get("op", {}).get("type") == "turn_begin" and e["op"].get("turn_id") == turn_id
        ]
        ended = [
            e for e in ops
            if e.get("op", {}).get("type") == "turn_end" and e["op"].get("turn_id") == turn_id
        ]
        if begun and ended:
            turn_done = True
            turn_input = begun[0]["op"].get("input", "")
            break
        time.sleep(2)
    host_b.kill()
    if not turn_done:
        print(f"FAIL: fired turn {turn_id} did not complete in time")
        return 2
    if f"[scheduled by cron job {job_id}" not in turn_input:
        print(f"FAIL: fired turn input lacks provenance prefix: {turn_input[:120]!r}")
        return 2
    print(f"  fired turn {turn_id} completed with provenance-marked input")

    # ── Canary scan: the key must appear in NO captured artifact ──
    for artifact in ["host-a.log", "host-b.log"]:
        text = (work / artifact).read_text(encoding="utf-8", errors="replace")
        if key in text:
            print(f"FAIL: canary hit — key material found in {artifact}")
            return 2
    print("  canary scan: 0 key-material hits in captured logs")

    print("PASS: F-6 — job created through the cronjob tool (gate-prompted), "
          "journaled cron_created, killed host, resumed, fired exactly once")
    return 0


if __name__ == "__main__":
    sys.exit(main())
