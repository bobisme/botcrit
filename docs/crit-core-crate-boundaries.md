# crit Core Crate Boundaries and API Contract (Draft v1)

This note defines the target crate boundaries for the `crit` split into `crit-core`,
`crit-cli`, and `crit-tui`, plus a first-pass `crit-core` API surface.

Scope for this draft:

- boundary and dependency contract,
- module migration map from today's `src/` layout,
- typed service API sketch for shared review operations,
- error/output boundary strategy,
- identifier and serialization compatibility strategy,
- known API gaps required to replace CLI JSON shell-outs in UI integrations.

## 1) Dependency Graph Contract

Target workspace graph:

```text
crates/crit-core   <- no dependency on CLI or TUI crates
   ^         ^
   |         |
crates/crit-cli    crates/crit-tui
```

Hard rules:

1. `crit-core` is the only crate allowed to read/write `.crit/` event logs and projection state.
2. `crit-cli` and `crit-tui` may depend on `crit-core`, never the reverse.
3. `crit-cli` and `crit-tui` do not depend on each other.
4. Text/pretty/JSON presentation belongs in surface crates (`crit-cli`/`crit-tui`), not in `crit-core`.

## 2) Crate Responsibilities

### `crit-core`

Owns reusable domain behavior:

- event model + IDs (`events/*`),
- append-log storage (`log/*`),
- projection schema + queries (`projection/*`),
- sync/rebuild/orphan handling,
- repository context and diff/context helpers (`jj/*`, later SCM abstraction),
- domain-level service operations used by both CLI and TUI.

Must not include:

- clap argument parsing,
- terminal formatting (`Formatter`) and CLI guidance strings,
- ratatui rendering/event loop concerns.

### `crit-cli`

Owns transport and presentation:

- clap command graph and argument parsing,
- output modes (`text`, `pretty`, `json`),
- user-facing command guidance and actionable remediation text,
- process exit code policy.

### `crit-tui`

Owns interactive UX:

- app state, key handling, layout/rendering,
- focused workflows (review browser, diff/thread panes),
- polling/refresh cadence and interaction-specific state.

`crit-tui` must call typed `crit-core` services directly for data and mutations.

## 3) Module Migration Map

Current monolith modules and proposed destination:

| Current path | Destination crate | Notes |
| --- | --- | --- |
| `src/events/*` | `crit-core` | Unchanged ownership |
| `src/log/*` | `crit-core` | Unchanged ownership |
| `src/projection/*` | `crit-core` | Unchanged ownership |
| `src/jj/*` | `crit-core` | Keep JJ-specific helpers in core for now |
| `src/critignore.rs` | `crit-core` | Shared filtering behavior |
| `src/version.rs` | `crit-core` | Data format versioning |
| `src/output/*` | `crit-cli` | CLI formatting only |
| `src/cli/*` | `crit-cli` | CLI command transport only |
| `src/tui/*` | `crit-tui` | TUI UX only |
| `src/main.rs` | `crit-cli` | Binary entrypoint |

Planned internal module shape in `crit-core` (first pass):

- `core::services::{reviews,threads,comments,inbox,sync,status}`
- `core::types::{requests,responses,errors}`
- `core::repo::{workspace,context,diff}` (initially JJ-backed)

## 4) First-Pass `crit-core` API Surface

This section is intentionally concrete so extraction can proceed without re-deciding
method boundaries in each task.

```rust
pub struct CoreContext {
    pub crit_root: std::path::PathBuf,
    pub workspace_root: std::path::PathBuf,
}

pub struct CoreServices {
    pub reviews: ReviewService,
    pub threads: ThreadService,
    pub comments: CommentService,
    pub inbox: InboxService,
    pub sync: SyncService,
    pub status: StatusService,
}
```

### Review operations

```rust
pub trait ReviewService {
    fn create_review(&self, req: CreateReview) -> CoreResult<ReviewCreatedResult>;
    fn list_reviews(&self, query: ReviewListQuery) -> CoreResult<Vec<ReviewSummary>>;
    fn get_review(&self, review_id: &str) -> CoreResult<ReviewDetail>;
    fn request_reviewers(&self, req: RequestReviewers) -> CoreResult<ReviewersRequestedResult>;
    fn vote(&self, req: ReviewVote) -> CoreResult<ReviewVoteResult>;
    fn approve(&self, req: ApproveReview) -> CoreResult<ReviewApprovedResult>;
    fn abandon(&self, req: AbandonReview) -> CoreResult<ReviewAbandonedResult>;
    fn mark_merged(&self, req: MarkMerged) -> CoreResult<ReviewMergedResult>;
    fn get_review_activity(&self, query: ReviewActivityQuery) -> CoreResult<ReviewActivity>;
}
```

### Thread operations

```rust
pub trait ThreadService {
    fn create_thread(&self, req: CreateThread) -> CoreResult<ThreadCreatedResult>;
    fn list_threads(&self, query: ThreadListQuery) -> CoreResult<Vec<ThreadSummary>>;
    fn get_thread(&self, query: ThreadDetailQuery) -> CoreResult<ThreadDetailWithContext>;
    fn resolve_threads(&self, req: ResolveThreads) -> CoreResult<ThreadResolutionResult>;
    fn reopen_thread(&self, req: ReopenThread) -> CoreResult<ThreadReopenedResult>;
}
```

### Comment operations

```rust
pub trait CommentService {
    fn add_comment(&self, req: AddComment) -> CoreResult<CommentAddedResult>;
    fn comment_or_create_thread(&self, req: CommentOnReview) -> CoreResult<CommentAddedResult>;
    fn list_comments(&self, thread_id: &str) -> CoreResult<Vec<Comment>>;
}
```

### Inbox + sync + status operations

```rust
pub trait InboxService {
    fn get_inbox(&self, agent: &str) -> CoreResult<InboxSummary>;
}

pub trait SyncService {
    fn sync(&self) -> CoreResult<SyncReport>;
    fn rebuild(&self) -> CoreResult<RebuildReport>;
    fn accept_regression(&self, review_id: &str) -> CoreResult<SyncReport>;
}

pub trait StatusService {
    fn review_status(&self, query: ReviewStatusQuery) -> CoreResult<Vec<ReviewStatus>>;
    fn review_diff(&self, query: ReviewDiffQuery) -> CoreResult<ReviewDiffResult>;
}
```

Notes:

- Existing projection query types (`ReviewSummary`, `ReviewDetail`, `ThreadSummary`, `ThreadDetail`,
  `Comment`, `InboxSummary`) remain canonical DTOs unless a compatibility reason requires wrappers.
- `get_review_activity` maps to what CLI `review` currently builds (review + threads + comments + optional context/diffs).

## 5) Error and Output Boundary Strategy

`crit-core` returns typed errors; surface crates translate to user-facing text.

```rust
pub type CoreResult<T> = Result<T, CoreError>;

pub enum CoreError {
    NotInitialized,
    NotFound { entity: EntityKind, id: String },
    InvalidInput { field: &'static str, message: String },
    InvalidState { entity: EntityKind, id: String, state: String },
    VersionMismatch { expected: &'static str, found: String },
    Conflict { message: String },
    Storage { message: String },
    Projection { message: String },
    Repo { message: String },
}
```

Rules:

1. `crit-core` never emits CLI copy such as `"To fix: crit ..."`.
2. `crit-cli` maps `CoreError` to command-specific actionable guidance.
3. `crit-tui` maps `CoreError` to actionable in-app status/toast content.
4. Formatting (`text`/`pretty`/`json`) remains exclusively in `crit-cli`.

## 6) Identifier and Serialization Compatibility Strategy

Compatibility requirements:

1. Event schema in `.crit/reviews/*/events.jsonl` remains backward-compatible.
2. ID formats remain unchanged:
   - `cr-*` for reviews,
   - `th-*` for threads,
   - `th-*.N` for comments.
3. Projection DB remains rebuildable from logs; no new source of truth introduced.
4. Existing CLI JSON contract remains stable during migration.

Plan:

- Keep `events::EventEnvelope` and all existing event payload structs in `crit-core`.
- Keep projection query structs in `crit-core` and reuse from both CLI and TUI.
- If internal renames are needed, add explicit serde field aliases/deprecations at the CLI boundary,
  with parity tests for `--format json` outputs on key commands.

## 7) API Gaps for Replacing CLI JSON Shell-Outs

The following capabilities are currently assembled in CLI handlers and must exist as
direct `crit-core` APIs for UI consumers:

1. **Aggregated review payload**
   - Equivalent of `run_review` JSON output: review, threads, comments, optional context,
     and optional per-file diff/content windows.

2. **Per-file diff packaging with orphan thread support**
   - CLI currently splits one git-format diff into file sections and provides content windows
     when thread anchors no longer intersect active hunks.

3. **Status + drift computation as typed data**
   - Equivalent of `run_status` including drift state per thread.

4. **Auto-thread comment workflow**
   - `comment` behavior (create thread if absent, otherwise append comment) must be exposed as
     a single core mutation API.

5. **Action-oriented error classification**
   - CLI string errors should become structured core errors so TUI can render meaningful guidance
     without parsing command stderr.

6. **Sync and anomaly visibility**
   - `sync`, `rebuild`, and regression acceptance behaviors must be callable directly with typed
     anomaly reports.

## 8) Explicit Non-Goals (for this migration)

1. Changing on-disk review ownership model (`.crit/` remains in-repo source of truth).
2. Rewriting event history format or ID schemes.
3. Moving UI rendering concerns into `crit-core`.
4. Requiring subprocess calls for core read/write paths once `crit-core` APIs exist.
5. Introducing network services or external review storage.

## 9) Execution Readiness Checklist

- [x] crate dependency direction is explicit and enforceable,
- [x] migration map from current modules is explicit,
- [x] first-pass `crit-core` service surface is concrete enough for implementation,
- [x] output/error boundary ownership is explicit,
- [x] non-goals are explicit,
- [x] UI integration gaps are identified and tied to current command behavior.

This document is the implementation contract for `bn-1kpr` and the dependency input
for workspace extraction work in `bn-2gdf`.
