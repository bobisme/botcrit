# crit

Project type: cli
Tools: `beads`, `maw`, `crit`, `botbus`, `botty`
Reviewer roles: security

Distributed code review for jj, built for AI agent teams.
Reviews live in `.crit/` alongside the code — no server, no accounts, no network.
Review lifecycle: create -> comment/vote -> approve -> merge.

## Architecture

Event-sourced.

```
Source of truth:  .crit/reviews/{review_id}/events.jsonl  (append-only JSONL, one per review)
Projection:       .crit/index.db  (SQLite cache, gitignored, rebuildable from logs)
```

Reviews anchored to jj Change IDs (survive rebases). `.crit/` is version-controlled — reviews travel with code. **Never propose moving event storage out of the repo.**

### Source Layout

| Layer      | Key files                                                                                     | Purpose                                                     |
| ---------- | --------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| CLI        | `cli/mod.rs`, `cli/commands/{reviews,threads,comments,init,migrate,status,doctor,helpers}.rs` | Clap defs + command handlers                                |
| Events     | `events/mod.rs`, `events/ids.rs`, `events/identity.rs`                                        | Event types, terseid IDs (`cr-`, `th-`), agent identity     |
| Storage    | `log/mod.rs`                                                                                  | `AppendLog`/`ReviewLog`, fs2 file locking, atomic appends   |
| Projection | `projection/mod.rs`, `projection/query.rs`                                                    | SQLite schema, sync, orphan/truncation detection, query API |
| JJ         | `jj/mod.rs`, `jj/context.rs`, `jj/drift.rs`                                                   | Workspace/repo root resolution, code context, line drift    |
| Output     | `output/mod.rs`                                                                               | text, pretty, and JSON formatters                           |
| TUI        | `tui/{app,ui,theme}.rs`, `tui/views/`                                                         | Ratatui interactive browser                                 |
| Other      | `critignore.rs`, `version.rs`                                                                 | .critignore patterns, v1/v2 format detection                |

### Data Model

**Events** (in `EventEnvelope` with timestamp + author):

- Review: `ReviewCreated`, `ReviewersRequested`, `ReviewerVoted`(LGTM/Block), `ReviewApproved`, `ReviewMerged`, `ReviewAbandoned`
- Thread: `ThreadCreated`(has review_id+thread_id), `ThreadResolved`, `ThreadReopened`(thread_id only)
- Comment: `CommentAdded`(comment_id=`th-xxx.N`, thread_id only)

**IDs**: terseid-based. `cr-xxxx` (reviews), `th-xxxx` (threads), `th-xxxx.N` (comments). `generate_valid_id()` retries until parse rules pass (4+ char hashes need a digit).

**States**: `open` -> `approved` (auto on LGTM if no blocks) -> `merged` | `open` -> `abandoned`

**Inbox** (3 categories): awaiting-vote (`[fresh]`/`[re-review]`), new thread responses, open feedback on your reviews.

### Key Constraints

- `crit comment` writes ThreadCreated + CommentAdded as two events. ThreadCreated links thread_id->review_id; CommentAdded/ThreadResolved/ThreadReopened only carry thread_id.
- jj operations (squash, rebase, workspace merge) can restore stale event logs. Detected via truncation check + content hash. Lost reviews saved to `.crit/orphaned-reviews-*.json`. Recovery: `jj file annotate .crit/reviews/{id}/events.jsonl`.
- Every command needs identity: `--agent`, `CRIT_AGENT`, or `BOTBUS_AGENT` env var.
- Output: text (default, compact) or `--format json|text|pretty`. All output must be self-contained with actionable next-steps. Errors include fix commands.

### Testing

`cargo test` runs unit tests (`#[cfg(test)]` in modules). Key coverage: ID generation (500-iteration stress test), event serialization, projection sync/orphan detection, critignore, init idempotence. Integration tests use unique socket paths for botty (`/tmp/botty-test-{uuid}.sock`). Debug: `RUST_LOG=debug cargo test -- --nocapture`.

<!-- botbox:managed-start -->
## Botbox Workflow

**New here?** Read [worker-loop.md](.agents/botbox/worker-loop.md) first — it covers the complete triage → start → work → finish cycle.

**All tools have `--help`** with usage examples. When unsure, run `<tool> --help` or `<tool> <command> --help`.

### IMPORTANT: Always Track Work in Beads

**Every non-trivial change MUST have a bead**, no matter how it originates:
- **User asks you to do something** → create a bead before starting
- **You propose a change** → create a bead before starting
- **Mid-conversation pivot to implementation** → create a bead before coding

The only exceptions are truly microscopic changes (typo fixes, single-line tweaks) or when you are already iterating on an existing bead's implementation.

Without a bead, work cannot be recovered from crashes, handed off to other agents, or tracked for review. When in doubt, create the bead — it takes seconds and prevents lost work.

### Directory Structure (maw v2)

This project uses a **bare repo** layout. Source files live in workspaces under `ws/`, not at the project root.

```
project-root/          ← bare repo (no source files here)
├── ws/
│   ├── default/       ← main working copy (AGENTS.md, .beads/, src/, etc.)
│   ├── frost-castle/  ← agent workspace (isolated jj commit)
│   └── amber-reef/    ← another agent workspace
├── .jj/               ← jj repo data
├── .git/              ← git data (core.bare=true)
├── AGENTS.md          ← stub redirecting to ws/default/AGENTS.md
└── CLAUDE.md          ← symlink → AGENTS.md
```

**Key rules:**
- `ws/default/` is the main workspace — beads, config, and project files live here
- **Never merge or destroy the default workspace.** It is where other branches merge INTO, not something you merge.
- Agent workspaces (`ws/<name>/`) are isolated jj commits for concurrent work
- **ALL commands must go through `maw exec`** — this includes `br`, `bv`, `crit`, `jj`, `cargo`, `bun`, and any project tool. Never run them directly from the bare repo root.
- Use `maw exec default -- <command>` for beads (`br`, `bv`) and general project commands
- Use `maw exec <agent-ws> -- <command>` for workspace-scoped commands (`crit`, `jj describe`, `cargo check`)
- **crit commands must run in the review's workspace**, not default: `maw exec <ws> -- crit ...`

### Beads Quick Reference

| Operation | Command |
|-----------|---------|
| View ready work | `maw exec default -- br ready` |
| Show bead | `maw exec default -- br show <id>` |
| Create | `maw exec default -- br create --actor $AGENT --owner $AGENT --title="..." --type=task --priority=2` |
| Start work | `maw exec default -- br update --actor $AGENT <id> --status=in_progress --owner=$AGENT` |
| Add comment | `maw exec default -- br comments add --actor $AGENT --author $AGENT <id> "message"` |
| Close | `maw exec default -- br close --actor $AGENT <id>` |
| Add dependency | `maw exec default -- br dep add --actor $AGENT <blocked> <blocker>` |
| Sync | `maw exec default -- br sync --flush-only` |
| Triage (scores) | `maw exec default -- bv --robot-triage` |
| Next bead | `maw exec default -- bv --robot-next` |

**Required flags**: `--actor $AGENT` on mutations, `--author $AGENT` on comments.

### Workspace Quick Reference

| Operation | Command |
|-----------|---------|
| Create workspace | `maw ws create <name>` |
| List workspaces | `maw ws list` |
| Merge to main | `maw ws merge <name> --destroy` |
| Destroy (no merge) | `maw ws destroy <name>` |
| Run jj in workspace | `maw exec <name> -- jj <jj-args...>` |

**Avoiding divergent commits**: Each workspace owns ONE commit. Only modify your own.

| Safe | Dangerous |
|------|-----------|
| `maw ws merge <agent-ws> --destroy` | `maw ws merge default --destroy` (NEVER) |
| `jj describe` (your working copy) | `jj describe main -m "..."` |
| `maw exec <your-ws> -- jj describe -m "..."` | `jj describe <other-change-id>` |

If you see `(divergent)` in `jj log`:
```bash
jj abandon <change-id>/0   # keep one, abandon the divergent copy
```

**Working copy snapshots**: jj auto-snapshots your working copy before most operations (`jj new`, `jj rebase`, etc.). Edits go into the **current** commit automatically. To put changes in a **new** commit, run `jj new` first, then edit files.

**Always pass `-m`**: Commands like `jj commit`, `jj squash`, and `jj describe` open an editor by default. Agents cannot interact with editors, so always pass `-m "message"` explicitly.

### Beads Conventions

- Create a bead before starting work. Update status: `open` → `in_progress` → `closed`.
- Post progress comments during work for crash recovery.
- **Push to main** after completing beads (see [finish.md](.agents/botbox/finish.md)).
- **Update CHANGELOG.md** when releasing: add a summary of user-facing changes under the new version heading before tagging.
- **Install locally** after releasing: `just install`

### Identity

Your agent name is set by the hook or script that launched you. Use `$AGENT` in commands.
For manual sessions, use `<project>-dev` (e.g., `myapp-dev`).

### Claims

When working on a bead, stake claims to prevent conflicts:

```bash
bus claims stake --agent $AGENT "bead://<project>/<id>" -m "<id>"
bus claims stake --agent $AGENT "workspace://<project>/<ws>" -m "<id>"
bus claims release --agent $AGENT --all  # when done
```

### Reviews

Create a review with reviewer assignment in one command, then @mention to spawn:

```bash
maw exec $WS -- crit reviews create --agent $AGENT --title "..." --description "..." --reviewers $PROJECT-security
bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request
```

For re-requests after fixing feedback, use `crit reviews request`:
```bash
maw exec $WS -- crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT
```

The @mention triggers the auto-spawn hook for the reviewer.

### Cross-Project Communication

**Don't suffer in silence.** If a tool confuses you or behaves unexpectedly, post to its project channel.

1. Find the project: `bus history projects -n 50` (the #projects channel has project registry entries)
2. Post question or feedback: `bus send --agent $AGENT <project> "..." -L feedback`
3. For bugs, create beads in their repo first
4. **Always create a local tracking bead** so you check back later:
   ```bash
   maw exec default -- br create --actor $AGENT --owner $AGENT --title="[tracking] <summary>" --labels tracking --type=task --priority=3
   ```

See [cross-channel.md](.agents/botbox/cross-channel.md) for the full workflow.

### Session Search (optional)

Use `cass search "error or problem"` to find how similar issues were solved in past sessions.


### Design Guidelines

- [CLI tool design for humans, agents, and machines](.agents/botbox/design/cli-conventions.md)

### Workflow Docs

- [Mission coordination labels and sibling awareness](.agents/botbox/coordination.md)
- [Ask questions, report bugs, and track responses across projects](.agents/botbox/cross-channel.md)
- [Close bead, merge workspace, release claims, sync](.agents/botbox/finish.md)
- [groom](.agents/botbox/groom.md)
- [Verify approval before merge](.agents/botbox/merge-check.md)
- [End-to-end mission lifecycle guide](.agents/botbox/mission.md)
- [Turn specs/PRDs into actionable beads](.agents/botbox/planning.md)
- [Validate toolchain health](.agents/botbox/preflight.md)
- [Create and validate proposals before implementation](.agents/botbox/proposal.md)
- [Report bugs/features to other projects](.agents/botbox/report-issue.md)
- [Reviewer agent loop](.agents/botbox/review-loop.md)
- [Request a review](.agents/botbox/review-request.md)
- [Handle reviewer feedback (fix/address/defer)](.agents/botbox/review-response.md)
- [Explore unfamiliar code before planning](.agents/botbox/scout.md)
- [Claim bead, create workspace, announce](.agents/botbox/start.md)
- [Find work from inbox and beads](.agents/botbox/triage.md)
- [Change bead status (open/in_progress/blocked/done)](.agents/botbox/update.md)
- [Full triage-work-finish lifecycle](.agents/botbox/worker-loop.md)
<!-- botbox:managed-end -->
