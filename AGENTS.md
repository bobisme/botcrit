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
- Use `maw exec <ws> -- <command>` to run commands in a workspace context
- Use `maw exec default -- br|bv ...` for beads commands (always in default workspace)
- Use `maw exec <ws> -- crit ...` for review commands (always in the review's workspace)
- Never run `br`, `bv`, `crit`, or `jj` directly — always go through `maw exec`

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

### Protocol Quick Reference

Use these commands at protocol transitions to check state and get exact guidance. Each command outputs instructions for the next steps.

| Step | Command | Who | Purpose |
|------|---------|-----|---------|
| Resume | `botbox protocol resume --agent $AGENT` | Worker | Detect in-progress work from previous session |
| Start | `botbox protocol start <bead-id> --agent $AGENT` | Worker | Verify bead is ready, get start commands |
| Review | `botbox protocol review <bead-id> --agent $AGENT` | Worker | Verify work is complete, get review commands |
| Finish | `botbox protocol finish <bead-id> --agent $AGENT` | Worker | Verify review approved, get close/cleanup commands |
| Merge | `botbox protocol merge <workspace> --agent $AGENT` | Lead | Check preconditions, detect conflicts, get merge steps |
| Cleanup | `botbox protocol cleanup --agent $AGENT` | Worker | Check for held resources to release |

All commands support JSON output with `--format json` for parsing. If a command is unavailable or fails (exit code 1), fall back to manual steps documented in [start](.agents/botbox/start.md), [review-request](.agents/botbox/review-request.md), and [finish](.agents/botbox/finish.md).

### Beads Conventions

- Create a bead before starting work. Update status: `open` → `in_progress` → `closed`.
- Post progress comments during work for crash recovery.
- **Run checks before requesting review**: `just check` (or your project's build/test command). Fix any failures before proceeding.
- After finishing a bead, follow [finish.md](.agents/botbox/finish.md). **Workers: do NOT push** — the lead handles merges and pushes.
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

Use `@<project>-<role>` mentions to request reviews:

```bash
maw exec $WS -- crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT
bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request
```

The @mention triggers the auto-spawn hook for the reviewer.

### Bus Communication

Agents communicate via bus channels. You don't need to be expert on everything — ask the right project.

| Operation | Command |
|-----------|---------|
| Send message | `bus send --agent $AGENT <channel> "message" [-L label]` |
| Check inbox | `bus inbox --agent $AGENT --channels <ch> [--mark-read]` |
| Wait for reply | `bus wait -c <channel> --mention -t 120` |
| Browse history | `bus history <channel> -n 20` |
| Search messages | `bus search "query" -c <channel>` |

**Conversations**: After sending a question, use `bus wait -c <channel> --mention -t <seconds>` to block until the other agent replies. This enables back-and-forth conversations across channels.

**Project experts**: Each `<project>-dev` is the expert on their project. When stuck on a companion tool (bus, maw, crit, botty, br), post a question to its project channel instead of guessing.

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


- [Find work from inbox and beads](.agents/botbox/triage.md)

- [Claim bead, create workspace, announce](.agents/botbox/start.md)

- [Change bead status (open/in_progress/blocked/done)](.agents/botbox/update.md)

- [Close bead, merge workspace, release claims, sync](.agents/botbox/finish.md)

- [Full triage-work-finish lifecycle](.agents/botbox/worker-loop.md)

- [Turn specs/PRDs into actionable beads](.agents/botbox/planning.md)

- [Explore unfamiliar code before planning](.agents/botbox/scout.md)

- [Create and validate proposals before implementation](.agents/botbox/proposal.md)

- [Request a review](.agents/botbox/review-request.md)

- [Handle reviewer feedback (fix/address/defer)](.agents/botbox/review-response.md)

- [Reviewer agent loop](.agents/botbox/review-loop.md)

- [Merge a worker workspace (protocol merge + conflict recovery)](.agents/botbox/merge-check.md)

- [Validate toolchain health](.agents/botbox/preflight.md)

- [Ask questions, report bugs, and track responses across projects](.agents/botbox/cross-channel.md)

- [Report bugs/features to other projects](.agents/botbox/report-issue.md)

- [groom](.agents/botbox/groom.md)

<!-- botbox:managed-end -->
