# Audit: NTFS hard-link escape against policy and DACL containment

Date: 2026-08-10
Scope: `nano-k3/crates/nano-core` (policy engine: `can_write_path_with_cwd`,
`is_write_link_escape`, `canonicalize_write_target`) and
`nano-k3/crates/nano-tools` (`FsTools` direct-tool read/write). OS layer
analyzed against the `nano-sandbox` grant model (read-only reference; not
modified). Threat model and decision criterion from
`shared/contracts/windows-hardlink-containment.md` (adopted Track A spike).
Method: live OS probes (Windows 11, 10.0.26200) plus pinned adversarial tests
in `nano-core/tests/adversarial_policy.rs` and
`nano-tools/tests/adversarial_fs.rs`.

## Verdict

**Hole found at the lexical policy layer; FIXED for writes. Reads through a
hard-link alias remain alias-transparent at the lexical layer — a documented
PLATFORM LIMITATION that the OS DACL layer contains (proven below).**

| Layer | Attack | Result | Status |
| --- | --- | --- | --- |
| Policy engine (write) | `fs::hard_link(outside\target, ws\link)` then write `ws\link` | Canonicalization saw a legitimate in-root path (no reparse point) → write ALLOWED, content landed in the outside object | **HOLE — FIXED** (link-count deny) |
| Tool layer (write) | same, via `FsTools::write_file` | Same hole (policy is the only gate at this layer) | **HOLE — FIXED** by the engine fix |
| Policy/tool layer (read) | hard link into a deny-read dir or onto a sensitive `.env`, read through the alias | `ReadDenyMatcher` and `is_sensitive_path` are name-based; the alias name is benign → read ALLOWED at this layer | **PLATFORM LIMITATION — documented; contained by the DACL layer** |
| OS DACL (create) | sandbox identity creates the alias itself | **DENIED**: since Windows 10, `CreateHardLinkW` requires write access to the TARGET; the sandbox identity has no write ACE on outside objects | Contained by design (proven) |
| OS DACL (write-through) | ambient same-user pre-plants the alias; sandbox writes through it | **DENIED**: the access check binds to the file OBJECT's DACL, not the directory entry; unchanged content verified | Contained by design (proven) |
| OS DACL (read-through) | same, read through a pre-planted alias | **DENIED** when the target DACL denies read (same object-level check) | Contained by design (proven) |

## Mechanism

A junction/symlink escape is visible to path canonicalization because the link
carries a reparse point: resolving the nearest existing ancestor rewrites the
path to the outside target, and `is_write_link_escape` denies it. An NTFS
**hard link** is different: it is a second directory entry (name) for the SAME
file object. There is no reparse point; `dunce::canonicalize` of the alias
returns the alias. Every lexical layer — prefix checks, canonicalization,
the read-deny matcher, the sensitive-basename check — sees a legitimate
in-workspace file. Before the fix, `can_write_path_with_cwd(ws\link.txt)`
returned `Write`, and `std::fs::write` through the alias truncated the outside
object in place (probe: `outside\target.txt` content changed from
`nano-probe-original` to `nano-probe-pwned`).

Two NTFS facts make the OS layer self-limiting for the sandbox identity
(both probed live, then pinned as Rust tests):

1. **Creation requires write access to the target.** Since Windows 10,
   `CreateHardLinkW` fails with `ERROR_ACCESS_DENIED` when the caller lacks
   write access to the existing file. The sandbox account/capability SIDs are
   granted ACEs only on writable roots; an outside target has no allow ACE
   for them, and an absent allow denies exactly like an explicit deny (the
   tests use an explicit self-deny ACE to emulate this deterministically).
2. **The DACL binds the object, not the name.** Opening ANY name of a file
   object access-checks the caller against that object's security descriptor.
   With a deny-write ACE on the target, writing through the in-workspace alias
   fails `ERROR_ACCESS_DENIED` and the content is unchanged; same for reads
   with a deny-read ACE. A pre-planted alias (the ambient same-user race from
   the spike) therefore does not extend the sandbox identity's authority.

What the DACL layer does NOT cover: a target the sandbox identity can
legitimately write (another writable root — aliasing one writable root into
another is contained anyway) and the **unsandboxed direct-tool path**, where
the process runs as the interactive user whose DACL allows both the workspace
and the outside file. That is exactly where the policy engine is the only
gate — and where the hole was.

## Fix

`FileSystemSandboxPolicy::can_write_path_with_cwd`
(`crates/nano-core/src/policy_engine.rs`) now denies writes to an existing
regular file with more than one hard link
(`existing_file_has_multiple_links`): Windows uses
`GetFileInformationByHandle` → `BY_HANDLE_FILE_INFORMATION.nNumberOfLinks`
via `windows-sys` 0.52 (the workspace-pinned version; stable std's
`MetadataExt::number_of_links` is still unstable, `windows_by_handle` #63010);
Unix uses stable `MetadataExt::nlink`. Missing targets (creates), directories
(not hard-linkable), and unopenable files are unaffected.

Rationale: an in-place write (`std::fs::write` truncates) through a
multi-linked name mutates the object under ALL its names, including names the
engine cannot see. Denying in-place writes to multi-linked files is the only
sound lexical decision. Callers that must "edit" such a file can unlink +
recreate, which breaks the alias instead of writing through it.

### Accepted false positives

- **In-root link pairs** (both names inside the writable root) are denied too:
  enumerating a file's other names (`FindFirstFileNameW`) was judged not worth
  the complexity; in-place mutation of a shared object is exactly what must
  not happen silently. Pinned by
  `in_workspace_hard_link_pair_is_also_write_denied`.
- **pnpm-style store links / local `git clone` object hard links**: files in
  `node_modules` linked from a global store, or packfiles shared between
  clones, become non-in-place-writable. This is the SAFE direction (writing
  through such a link would corrupt the store/clone outside the root); the
  deny is surfaced as a typed `WriteDenied`, never silent.

### Residual race (documented, not closed at this layer)

The link-count check is check-to-use: an ambient same-user process can plant
the alias AFTER the check and BEFORE the write. The spike
(`shared/contracts/windows-hardlink-containment.md`) already establishes that
no userspace check closes this race; the broker/VHDX design is the release
gate for the general shell. What this audit adds: the race is **harmless to
objects outside the sandbox identity's DACL grants** (proven above), so the
residual exposure is limited to objects the sandbox identity can already
write — plus the unsandboxed direct-tool path, where the adversary is the
agent's own tool call, not a concurrent process.

## Read side: platform limitation

`ReadDenyMatcher` and `is_sensitive_path` are name-based; a hard link named
`notes.txt` aliasing a denied `.env` defeats both at the lexical layer, and
denying ALL multi-linked reads is not viable (pnpm stores make multi-linked
reads common and legitimate). Reads are therefore NOT blocked at this layer;
the containment is the target object's DACL, which denies the sandbox
identity reads of anything outside its explicit read grants — including
through an alias (proven). Pinned as actual behavior by
`hard_link_read_alias_transparency_is_pinned_as_platform_limitation`
(nano-core) and
`read_through_hard_link_into_denied_dir_documents_alias_transparency`
(nano-tools).

## Test evidence

- `nano-core/tests/adversarial_policy.rs` (17 passed):
  `hard_link_alias_into_workspace_is_write_denied` (the fix pin — failed
  before the fix: the probe showed the write landing outside),
  `hard_link_write_deny_releases_when_outside_name_is_removed` (deny tracks
  link count, not name), `in_workspace_hard_link_pair_is_also_write_denied`
  (documented false positive),
  `hard_link_read_alias_transparency_is_pinned_as_platform_limitation`.
- `nano-tools/tests/adversarial_fs.rs` (13 passed):
  `write_through_hard_link_is_denied_and_does_not_land_outside`,
  `read_through_hard_link_into_denied_dir_documents_alias_transparency`,
  `hard_link_creation_to_write_denied_target_fails` (OS fact 1),
  `target_dacl_denies_write_through_preplanted_hard_link` (OS fact 2, raw
  `std::fs::write` bypassing the policy check on purpose — the DACL alone
  stops it; outside content verified unchanged),
  `target_dacl_denies_read_through_preplanted_hard_link` (read counterpart).

The DACL tests emulate the sandbox identity's missing grant with an explicit
self-deny ACE via `icacls` (an absent allow ACE denies identically; the
explicit form is deterministic under the interactive test token) and remove
the ACE before exit.
