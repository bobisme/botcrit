# Proposal: Support Git Repositories Without JJ (bd-1wwb)

## Context

`crit` is currently optimized for jj-based workflows. The core review model assumes a jj
change ID (`jj_change_id`) that survives rewrites, and the command layer calls `jj`
directly via `JjRepo`.

If we move away from jj, `crit` needs a first-class Git path that preserves the same
review UX (create/comment/vote/diff/status) without requiring jj to be installed.

## Problem Statement

Current jj coupling appears in three places:

1. **SCM command adapter** (`src/jj/mod.rs`, `src/jj/context.rs`, `src/jj/drift.rs`)
2. **Review identity model** (`ReviewCreated.jj_change_id` in `src/events/mod.rs`)
3. **Query/projection schema and read logic** (`src/projection/mod.rs`, `src/projection/query.rs`,
   and command handlers that resolve `jj_change_id` to a commit)

Without changes, `crit` cannot operate in a plain Git repository.

## Goals

1. Run all core review commands in Git repositories without jj.
2. Keep review data local and versioned in `.crit/` (no server dependency).
3. Preserve existing jj reviews and avoid forced data loss or manual recovery.
4. Keep output contracts stable for agents (`text`/`pretty`/`json`).
5. Minimize command behavior divergence between jj and git backends.

## Non-Goals

1. Replacing jj immediately in existing jj-first workflows.
2. Building a remote code-host integration (GitHub/GitLab APIs).
3. Solving branch policy/enforcement, permissions, or CI orchestration.

## Proposed Architecture

### 1) Introduce an SCM abstraction layer

Add a new module:

- `src/scm/mod.rs`
- `src/scm/git.rs`
- `src/scm/jj.rs` (wrapper over existing jj behavior)

Define a backend trait used by command handlers:

```rust
pub trait ScmRepo {
    fn kind(&self) -> ScmKind;
    fn root(&self) -> &Path;

    fn current_anchor(&self) -> Result<String>;      // change-like stable anchor
    fn current_commit(&self) -> Result<String>;      // commit hash
    fn commit_for_anchor(&self, anchor: &str) -> Result<String>;
    fn parent_commit(&self, commit: &str) -> Result<String>;

    fn diff_git(&self, from: &str, to: &str) -> Result<String>;
    fn diff_git_file(&self, from: &str, to: &str, file: &str) -> Result<String>;
    fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<String>>;

    fn file_exists(&self, rev: &str, path: &str) -> Result<bool>;
    fn show_file(&self, rev: &str, path: &str) -> Result<String>;
}
```

`main.rs` resolves backend once and passes a trait object (or enum wrapper) into command
handlers instead of constructing `JjRepo` directly.

### 2) Backend detection and selection

Default behavior:

1. If `--scm` is provided (`auto|git|jj`), honor it.
2. Else detect automatically:
   - jj repo/workspace present => jj backend
   - `.git` present => git backend
3. If both are present:
   - resolve both candidate roots and verify they refer to the same repository context
   - if roots mismatch, fail fast with actionable error
   - require explicit `--scm` (or `CRIT_SCM`) during transition instead of silently choosing one

Add optional env override: `CRIT_SCM=git|jj|auto`.

`crit doctor` should report:

- detected backends (`jj`, `git`, or both)
- resolved roots for each backend
- a warning/error when both exist but roots differ
- remediation command examples (`crit --scm jj ...` / `crit --scm git ...`)

### 3) Git backend command mapping

Git implementation should mirror current jj operations:

- current anchor: branch ref when available (`symbolic-ref --quiet HEAD`), fallback
  `detached:<commit>` in detached HEAD
- current commit: `git rev-parse HEAD`
- commit for anchor: `git rev-parse --verify --end-of-options <anchor>^{commit}`
- parent commit: `git rev-parse <commit>^` (handle root-commit edge case)
- diff: `git diff --no-color <from>..<to>` (or equivalent `--git`-style patch)
- changed files: `git diff --name-only <from>..<to>`
- file exists: `git cat-file -e <rev>:<path>`
- show file: `git show <rev>:<path>`

Keep output in standard unified diff format so existing parsers (`diff --git` split logic)
continue to work.

Validation requirements for git backend inputs:

- reject anchors beginning with `-` before invoking git commands
- pass all user/event-derived refs after `--end-of-options` where supported
- require refs to resolve to commits via `--verify`
- reject paths that are absolute, contain `..`, or are otherwise non-normalized

## Data Model and Schema Evolution

## Current limitation

`ReviewCreated` stores `jj_change_id`, and projection schema includes `reviews.jj_change_id`.
This is jj-specific.

## Proposed v3-compatible shape

Evolve review anchor to backend-neutral fields:

- `scm_kind` (`"jj" | "git"`)
- `scm_anchor` (jj change ID or git ref-like anchor)

Event evolution strategy:

1. Add new fields to `ReviewCreated`:
   - `scm_kind: Option<String>`
   - `scm_anchor: Option<String>`
2. Keep `jj_change_id` during transition for backwards compatibility.
3. Read path fallback:
   - if `scm_*` exists, use that
   - else treat as `scm_kind=jj`, `scm_anchor=jj_change_id`

Projection evolution:

1. Add `reviews.scm_kind TEXT NOT NULL DEFAULT 'jj'`
2. Add `reviews.scm_anchor TEXT`
3. Backfill existing rows from `jj_change_id`
4. Add index on `(scm_kind, scm_anchor)`

This avoids mandatory one-shot migration for users and keeps old event logs valid.

## Command-Level Changes

Update command handlers to depend on `ScmRepo` instead of `JjRepo`:

1. `reviews create`
   - use `current_anchor()` and `current_commit()`
2. `diff` and `status`
   - resolve target with `commit_for_anchor()` (or final commit)
3. `review` and `threads show --context`
   - use backend file retrieval for context windows
4. drift/status computations
   - reuse unified diff parser with backend-provided diffs

No output format changes are required for this phase.

## Security Invariants (Must Hold)

1. **No ambiguous backend in mixed repositories**
   - If both jj and git are detected with mismatched roots, fail and require explicit
     backend selection.

2. **No option/ref injection into SCM subprocesses**
   - Never pass unvalidated refs directly; enforce anchor validation + `--end-of-options`
     and commit verification.

3. **No path traversal via event data**
   - Treat `file_path` from events as untrusted input.
   - Enforce normalized relative repo paths only (no absolute paths, no `..`).

4. **Backend parity for validation**
   - Apply the same path/ref validation invariants to jj and git adapters to avoid
     backend-specific bypasses.

## Migration Plan

### Phase 0: Design + test harness

- Add backend-selection tests and fixture repos for both jj and git.
- Add command-level integration tests that run with each backend.

### Phase 1: Internal abstraction (no behavior change)

- Introduce `scm` module.
- Port current jj code into `scm::jj` adapter.
- Keep runtime default on jj.

### Phase 2: Git backend implementation

- Implement `scm::git`.
- Gate with `--scm git` and `CRIT_SCM=git` for early adopters.

### Phase 3: Data model broadening

- Add `scm_kind` and `scm_anchor` to events/projection.
- Keep backward-compatible read/write behavior.

### Phase 4: Default auto-detection

- Enable `auto` by default.
- Update `README.md`, `AGENTS.md`, and `crit doctor` output to report active backend.

### Phase 5: Cleanup (optional)

- Deprecate direct `jj_change_id` usage in internal APIs.
- Keep persisted field support for old review logs indefinitely (or document sunset window).

## Risks and Mitigations

1. **Git lacks jj-style stable change IDs**
   - Mitigation: store explicit `scm_anchor`; prefer branch ref when available;
     fallback to initial commit when anchor becomes invalid.

2. **Behavior drift between backends**
   - Mitigation: backend contract tests with same scenarios and expected outputs.

3. **Schema/event compatibility regressions**
   - Mitigation: dual-read path and snapshot tests with legacy event fixtures.

4. **Root detection confusion in mixed repos**
   - Mitigation: explicit `--scm` and clear doctor diagnostics.

5. **Agent workflow breakage**
   - Mitigation: keep command names and output envelopes stable; only backend internals change.

## Test Strategy

1. Unit tests for backend adapters (`scm::jj`, `scm::git`)
2. Golden tests for diff parsing and changed-file extraction on git output
3. Projection tests with mixed legacy (`jj_change_id`) and new (`scm_*`) events
4. Integration tests for:
   - `reviews create`
   - `diff`
   - `review --context`
   - `status`
   on both jj and git repos
5. `crit doctor` tests that verify backend reporting and actionable remediation hints
6. Security regression tests for:
   - mixed-backend root mismatch detection
   - anchors starting with `-` (rejected)
   - refs requiring `--end-of-options` handling
   - path traversal payloads in synthetic event logs (`../`, absolute paths)

## Acceptance Criteria

1. `crit` runs in a plain Git repo with no jj installed.
2. `reviews create`, `comment`, `diff`, `review`, `status`, `threads show` work under git.
3. Existing jj review logs remain readable and writable.
4. Output contracts remain stable for agent consumers.
5. `crit doctor` clearly reports active SCM backend and failure remediation steps.

## Suggested Bead Breakdown

1. Add `scm` abstraction and route command handlers through it.
2. Implement git backend command adapter.
3. Introduce backend-neutral anchor fields in events/projection.
4. Add dual-backend integration tests and fixtures.
5. Update docs and doctor output.
