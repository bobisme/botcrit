# crit

Project type: cli
Tools: `beads`, `maw`, `crit`, `botbus`, `botty`
Reviewer roles: security

<!-- Add project-specific context below: architecture, conventions, key files, etc. -->


### Using bv as an AI sidecar

bv is a fast terminal UI for Beads projects (.beads/issues.jsonl). It renders lists/details and precomputes dependency metrics (PageRank, critical path, cycles, etc.) so you instantly see blockers and execution order. Source of truth here is `.beads/issues.jsonl` (exported from `beads.db`); legacy `.beads/beads.jsonl` is deprecated and must not be used. For agents, it’s a graph sidecar: instead of parsing JSONL or risking hallucinated traversal, call the robot flags to get deterministic, dependency-aware outputs.

- bv --robot-help — shows all AI-facing commands.
- bv --robot-insights — JSON graph metrics (PageRank, betweenness, HITS, critical path, cycles) with top-N summaries for quick triage.
- bv --robot-plan — JSON execution plan: parallel tracks, items per track, and unblocks lists showing what each item frees up.
- bv --robot-priority — JSON priority recommendations with reasoning and confidence.
- bv --robot-recipes — list recipes (default, actionable, blocked, etc.); apply via bv --recipe <name> to pre-filter/sort before other flags.
- bv --robot-diff --diff-since <commit|date> — JSON diff of issue changes, new/closed items, and cycles introduced/resolved.

Use these commands instead of hand-rolling graph logic; bv already computes the hard parts so agents can act safely and quickly.

### ast-grep vs ripgrep (quick guidance)

**Use `ast-grep` when structure matters.** It parses code and matches AST nodes, so results ignore comments/strings, understand syntax, and can **safely rewrite** code.

- Refactors/codemods: rename APIs, change import forms, rewrite call sites or variable kinds.
- Policy checks: enforce patterns across a repo (`scan` with rules + `test`).
- Editor/automation: LSP mode; `--json` output for tooling.

**Use `ripgrep` when text is enough.** It’s the fastest way to grep literals/regex across files.

- Recon: find strings, TODOs, log lines, config values, or non‑code assets.
- Pre-filter: narrow candidate files before a precise pass.

**Rule of thumb**

- Need correctness over speed, or you’ll **apply changes** → start with `ast-grep`.
- Need raw speed or you’re just **hunting text** → start with `rg`.
- Often combine: `rg` to shortlist files, then `ast-grep` to match/modify with precision.

**Snippets**

Find structured code (ignores comments/strings):

```bash
ast-grep run -l TypeScript -p 'import $X from "$P"'
```

Codemod (only real `var` declarations become `let`):

```bash
ast-grep run -l JavaScript -p 'var $A = $B' -r 'let $A = $B' -U
```

Quick textual hunt:

```bash
rg -n 'console\.log\(' -t js
```

Combine speed + precision:

```bash
rg -l -t ts 'useQuery\(' | xargs ast-grep run -l TypeScript -p 'useQuery($A)' -r 'useSuspenseQuery($A)' -U
```

**Mental model**

- Unit of match: `ast-grep` = node; `rg` = line.
- False positives: `ast-grep` low; `rg` depends on your regex.
- Rewrites: `ast-grep` first-class; `rg` requires ad‑hoc sed/awk and risks collateral edits.

## Testing Strategy for botty

botty is a PTY-based daemon with Unix socket IPC. Testing requires care around process lifecycle and socket cleanup.

### Test Categories

1. **Unit tests** (`#[cfg(test)]` in modules)
   - Protocol serialization/deserialization roundtrips
   - Transcript ring buffer operations
   - Screen normalization logic
   - Name generation uniqueness

2. **Integration tests** (`tests/` directory)
   - Server startup/shutdown lifecycle
   - Full request/response cycles over Unix socket
   - Agent spawn → send → snapshot → kill flows
   - **Socket cleanup**: Each test should use a unique socket path (e.g., `/tmp/botty-test-{uuid}.sock`)

3. **End-to-end CLI tests**
   - Run actual `cargo run -- spawn`, `cargo run -- list`, etc.
   - Use `assert_cmd` crate for ergonomic CLI testing
   - Verify exit codes and stdout/stderr

### Running Tests

```bash
# Unit + integration tests
cargo test

# With logging for debugging
RUST_LOG=debug cargo test -- --nocapture

# Single test
cargo test test_name

# Integration tests only
cargo test --test '*'
```

### Manual Testing Checklist

For attach mode and interactive features that are hard to automate:

```bash
# 1. Basic spawn and interaction
botty spawn -- bash
botty list
botty send <id> "echo hello"
botty tail <id>
botty snapshot <id>
botty kill <id>

# 2. Attach mode
botty spawn -- bash
botty attach <id>
# Type commands, verify they work
# Press Ctrl+A then 'd' to detach
# Verify you're back at your shell

# 3. TUI program
botty spawn -- htop
botty snapshot <id>  # Should show htop UI
botty attach <id>    # Should be interactive
```

### Test Fixtures

For deterministic snapshot testing, use simple programs with predictable output:

```bash
# Spawn a program that prints known output
botty spawn -- sh -c 'echo "line1"; echo "line2"; sleep 999'
botty snapshot <id>  # Compare against expected

# Test screen handling with cursor movement
botty spawn -- sh -c 'printf "ABC\rX"; sleep 999'
botty snapshot <id>  # Should show "XBC"
```

### Session End Checklist

**IMPORTANT: Always commit your work before ending a session!**

```bash
# 1. Run tests to verify nothing is broken
cargo test

# 2. Check what's changed
jj status
jj diff --stat

# 3. Commit with a descriptive message
jj commit -m "feat(scope): description

Co-Authored-By: Claude <noreply@anthropic.com>"

# 4. Sync beads if you modified issues
br sync --flush-only

# 5. Verify commit history looks good
jj log --limit 5
```

Don't leave uncommitted work - it makes handoffs difficult and risks losing progress.

### Commit Conventions

Use [semantic commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer]
Co-Authored-By: Claude <noreply@anthropic.com>
```

**Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

**Examples**:

```bash
git commit -m "docs(jj): add workspace documentation for parallel agents

Co-Authored-By: Claude <noreply@anthropic.com>"

git commit -m "fix(tui): correct mouse click handling in popups

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Always include the `Co-Authored-By` trailer when Claude contributed to the work.

<!-- orchestrator-subagent-instructions-v1 -->

## Orchestrator: Spawning Subagents

When spawning subagents via the Task tool for parallel work, include these instructions in your prompt:

### Subagent Prompt Template

```
You are a subagent working on [TASK DESCRIPTION].

**IMPORTANT: Multi-agent coordination required.**

Before starting work:
1. Set your botbus identity:
   export BOTBUS_AGENT=[AGENT_NAME]

2. Create your maw workspace:
   maw ws create [WORKSPACE_NAME]
   cd /home/bob/src/botcrit/.workspaces/[WORKSPACE_NAME]

3. Announce your work:
   botbus send general "Starting: [TASK]"

4. Claim files you'll edit:
   botbus claim "src/[path]/**" -m "[TASK]"

During work:
- Work ONLY in your workspace directory
- Check botbus history if you need to coordinate
- Send updates on blockers or questions

When done:
1. Run tests: cargo test
2. Describe your commit: jj describe -m "[commit message]"
3. Announce completion: botbus send general "Done: [TASK]"
4. Release claims: botbus release --all

Do NOT merge - the orchestrator will merge all workspaces.

[REST OF TASK-SPECIFIC INSTRUCTIONS]
```

### Workspace Naming Convention

- Use task ID when available: `agent-bd-xxx`
- Or descriptive name: `agent-jj-wrapper`, `agent-projection`

### Merging Subagent Work

After all subagents complete:

```bash
# Check workspace status
maw ws status

# Merge all workspaces
maw ws merge --all

# Or merge and destroy (use --force to skip confirmation)
maw ws merge --all --destroy

# Run tests on merged result
cargo test
```

### Notes on Workspace Behavior

- Workspaces branch from `@` (current working copy), so subagents see uncommitted orchestrator work
- Each workspace starts with an empty "wip" commit - this is intentional to prevent divergent commits
- Use `--force` with `maw ws destroy` to skip confirmation in scripts

### When to Use Subagents

Use subagents with maw workspaces when:

- Multiple agents will edit files in parallel
- Work can be cleanly separated by module/directory
- Tasks are independent and non-blocking

Don't bother with workspaces when:

- Single agent doing sequential work
- Quick tasks that complete in one shot
- Research/read-only tasks

### Avoiding Merge Conflicts

If multiple agents need to touch the same file (e.g., adding modules to `mod.rs`):

1. **Pre-create shared structure** - Before spawning agents, add the module declarations yourself:

   ```rust
   // Orchestrator adds this before spawning agents:
   pub mod drift;
   pub mod context;
   ```

   Then agents only create their own files (drift.rs, context.rs).

2. **Combine related work** - If tasks touch the same directory, consider one agent for both.

3. **Sequential for shared files** - Spawn agents one at a time when they need to modify the same file.

Conflict resolution belongs in jj, not maw. If you get conflicts, use `jj status` to see them and resolve manually.

<!-- end-orchestrator-subagent-instructions -->

<!-- crit-agent-instructions -->

## Crit: Agent-Centric Code Review

This project uses [crit](https://github.com/anomalyco/botcrit) for distributed code reviews optimized for AI agents.

### Quick Start

```bash
# Initialize crit in the repository (once)
crit init

# Create a review for current change
crit reviews create --title "Add feature X"

# List open reviews
crit reviews list

# Check reviews needing your attention
crit reviews list --needs-review --author $BOTBUS_AGENT

# Show review details
crit reviews show <review_id>
```

### Adding Comments (Recommended)

The simplest way to comment on code - auto-creates threads:

```bash
# Add a comment on a specific line (creates thread automatically)
crit comment <review_id> --file src/main.rs --line 42 "Consider using Option here"

# Add another comment on same line (reuses existing thread)
crit comment <review_id> --file src/main.rs --line 42 "Good point, will fix"

# Comment on a line range
crit comment <review_id> --file src/main.rs --line 10-20 "This block needs refactoring"
```

### Replying to Threads

Use `crit reply` to respond to an existing thread (instead of `crit comment` which creates new threads):

```bash
# Reply to an existing thread
crit reply <thread_id> "Good point, will fix"
```

### Managing Threads

```bash
# List threads on a review
crit threads list <review_id>

# Show thread with context
crit threads show <thread_id>

# Resolve a thread
crit threads resolve <thread_id> --reason "Fixed in latest commit"
```

### Voting on Reviews

```bash
# Approve a review (LGTM)
crit lgtm <review_id> -m "Looks good!"

# Block a review (request changes)
crit block <review_id> -r "Need more test coverage"
```

### Viewing Full Reviews

```bash
# Show full review with all threads and comments
crit review <review_id>

# Show with more context lines
crit review <review_id> --context 5

# List threads with first comment preview
crit threads list <review_id> -v
```

### Approving and Merging

```bash
# Approve a review (changes status to approved)
crit reviews approve <review_id>

# Mark as merged (after jj squash/merge)
# Note: Will fail if there are blocking votes
crit reviews merge <review_id>

# Self-approve and merge in one step (solo workflows)
crit reviews merge <review_id> --self-approve
```

### Checking Your Inbox

```bash
# See all items needing your attention
crit inbox

# Shows:
# - Reviews awaiting your vote (you're a reviewer but haven't voted)
# - Threads with new responses (someone replied to your comment)
# - Open feedback on your reviews (threads others opened on your code)
```

### Reviewer Re-Request Workflow

When an author makes changes after receiving feedback, they can re-request review:

```bash
# Author re-requests review after addressing feedback
crit reviews request <review_id> --reviewers <reviewer-name>
```

**How it works:**

1. **Initial request**: Review appears in reviewer's inbox as `[fresh]`
2. **After voting**: Review disappears from inbox (reviewer has voted)
3. **Re-request**: Author runs `crit reviews request` to notify reviewer of changes
4. **Re-review**: Review reappears in inbox as `[re-review]`

**Inbox status indicators:**

- `[fresh]` — First-time review request (never voted)
- `[re-review]` — Author re-requested after you already voted

**Example workflow:**

```bash
# Reviewer sees in inbox:
crit inbox
# → cr-abc · Feature X by author-agent [re-review]

# Reviewer checks the updated diff and votes
crit diff cr-abc
crit lgtm cr-abc -m "Changes look good"
```

### Agent Best Practices

1. **Set your identity** via environment:

   ```bash
   export BOTBUS_AGENT=my-agent-name
   ```

2. **Check inbox at session start**:

   ```bash
   crit inbox
   ```

3. **Check status** to see unresolved threads:

   ```bash
   crit status <review_id> --unresolved-only
   ```

4. **Run doctor** to verify setup:

   ```bash
   crit doctor
   ```

### Output Formats

- Default output is TOON (token-optimized, human-readable)
- Use `--json` flag for machine-parseable JSON output

### Key Concepts

- **Reviews** are anchored to jj Change IDs (survive rebases)
- **Threads** group comments on specific file locations
- **crit comment** leaves feedback on a file+line (auto-creates threads)
- **crit reply** responds to an existing thread
- Works across jj workspaces (shared .crit/ in main repo)

### Architecture Constraint: Reviews Live With Code

**Hard requirement**: Review data (`.crit/events.jsonl`) MUST be checked into the repository. This is a core design principle of crit - reviews travel with the code, are versioned with the code, and can be examined historically.

**Consequence**: jj working copy operations (squash, rebase, workspace merge) can replace `events.jsonl` with an older version, causing review data loss.

**Mitigations in place** (these detect loss, they don't prevent it):
- Truncation detection: rebuilds projection if file is shorter than expected
- Content hash detection: rebuilds if file content changed
- Orphan backup: saves lost review IDs to `.crit/orphaned-reviews-{timestamp}.json`
- Recovery hints: points to `jj file annotate .crit/events.jsonl` for history

**When reviews are lost**: Check `.crit/orphaned-reviews-*.json` for affected review IDs, then use `jj file annotate .crit/events.jsonl` to find and restore from history.

**Do NOT propose**: Moving `events.jsonl` to external storage (~/.local/share, .jj/, etc.). This defeats the core value proposition of crit.

### Output Guidelines

Crit is frequently invoked by agents with **no prior context**. Every piece of tool output must be self-contained and actionable.

**Errors** must include:
- What failed (include details when available)
- How to fix it (exact command to run)
- Example: `"Review not found: cr-abc\n  To fix: crit reviews list"`

**Success output** must include:
- What happened
- What to do next (exact commands)
- Example: `"Review cr-abc created!\n  Next: crit comment cr-abc --file src/main.rs --line 10 \"feedback\""`

**Principles**:
- Agents can't remember prior output — every message must stand alone
- Include copy-pasteable commands, not just descriptions
- Keep it brief — agents are token-conscious
- Use structured prefixes where appropriate: `WARNING:`, `IMPORTANT:`, `To fix:`, `Next:`
- Assume agents have **zero crit knowledge** — every concept (threads, reviews, TOON, change IDs) needs a one-line explanation the first time it appears in a given output context
- All file paths in output should be relative to repo root for clarity

<!-- end-crit-agent-instructions -->

## Release Process

### Version Bumps

Use semantic versioning:

- **MAJOR** (1.0.0): Breaking changes
- **MINOR** (0.X.0): New features, backward compatible
- **PATCH** (0.0.X): Bug fixes, minor improvements

### Release Checklist

```bash
# 1. Ensure tests pass
just test

# 2. Bump version in Cargo.toml
#    Edit: version = "X.Y.Z"

# 3. Commit the release
jj commit -m "chore: bump version to X.Y.Z

Co-Authored-By: Claude <noreply@anthropic.com>"

# 4. Create version tag
jj tag set vX.Y.Z -r @-

# 5. Update main bookmark and push (including tags)
jj bookmark set main -r @-
jj git push --all

# 6. Install locally
just install

# 7. Verify
crit --version

# 8. Announce on botbus
botbus --agent botcrit-dev send botcrit "crit vX.Y.Z released - [summary of changes]"
```

### Quick Reference

| Stage    | Commands                                           |
| -------- | -------------------------------------------------- |
| Test     | `just test`                                        |
| Bump     | Edit `Cargo.toml` version                          |
| Commit   | `jj commit -m "chore: bump version to X.Y.Z"`      |
| Tag      | `jj tag set vX.Y.Z -r @-`                          |
| Push     | `jj bookmark set main -r @- && jj git push --all`  |
| Install  | `just install`                                     |
| Announce | `botbus send botcrit "crit vX.Y.Z - ..."`          |


<!-- botbox:managed-start -->
## Botbox Workflow

This project uses the botbox multi-agent workflow.

### Identity

Every command that touches bus or crit requires `--agent <name>`.
Use `<project>-dev` as your name (e.g., `terseid-dev`). Agents spawned by `agent-loop.sh` receive a random name automatically.
Run `bus whoami --agent $AGENT` to confirm your identity.

### Lifecycle

**New to the workflow?** Start with [worker-loop.md](.agents/botbox/worker-loop.md) — it covers the complete triage → start → work → finish cycle.

Individual workflow docs:

- [Close bead, merge workspace, release claims, sync](.agents/botbox/finish.md)
- [groom](.agents/botbox/groom.md)
- [Verify approval before merge](.agents/botbox/merge-check.md)
- [Validate toolchain health](.agents/botbox/preflight.md)
- [Report bugs/features to other projects](.agents/botbox/report-issue.md)
- [Reviewer agent loop](.agents/botbox/review-loop.md)
- [Request a review](.agents/botbox/review-request.md)
- [Handle reviewer feedback (fix/address/defer)](.agents/botbox/review-response.md)
- [Claim bead, create workspace, announce](.agents/botbox/start.md)
- [Find work from inbox and beads](.agents/botbox/triage.md)
- [Change bead status (open/in_progress/blocked/done)](.agents/botbox/update.md)
- [Full triage-work-finish lifecycle](.agents/botbox/worker-loop.md)

### Quick Start

```bash
AGENT=<project>-dev   # or: AGENT=$(bus generate-name)
bus whoami --agent $AGENT
br ready
```

### Beads Conventions

- Create a bead for each unit of work before starting.
- Update status as you progress: `open` → `in_progress` → `closed`.
- Reference bead IDs in all bus messages.
- Sync on session end: `br sync --flush-only`.
- **Always push to main** after completing beads (see [finish.md](.agents/botbox/finish.md)).
- **Release after features/fixes**: If the batch includes user-visible changes (not just chores), follow the project's release process (version bump → tag → announce).

### Beads Quick Reference

Beads are **project-local** — always `cd` to the project directory first.

| Operation | Command |
|-----------|---------|
| View ready work | `br ready` |
| Show bead | `br show <id>` |
| Create | `br create --actor $AGENT --owner $AGENT --title="..." --type=task --priority=2` |
| Start work | `br update --actor $AGENT <id> --status=in_progress` |
| Add comment | `br comments add --actor $AGENT --author $AGENT <id> "message"` |
| Close | `br close --actor $AGENT <id>` |
| Add labels | `br update --actor $AGENT <id> --labels=foo,bar` |
| Add dependency | `br dep add --actor $AGENT <blocked> <blocker>` |
| Block | `br update --actor $AGENT <id> --status=blocked` |
| Sync | `br sync --flush-only` |

**Required flags**: `--actor $AGENT` on all mutations, `--author $AGENT` on comments.

### Mesh Protocol

- Include `-L mesh` on bus messages.
- Claim bead: `bus claims stake --agent $AGENT "bead://$BOTBOX_PROJECT/<bead-id>" -m "<bead-id>"`.
- Claim workspace: `bus claims stake --agent $AGENT "workspace://$BOTBOX_PROJECT/$WS" -m "<bead-id>"`.
- Claim agents before spawning: `bus claims stake --agent $AGENT "agent://role" -m "<bead-id>"`.
- Release claims when done: `bus claims release --agent $AGENT --all`.

### Spawning Agents

1. Check if the role is online: `bus agents`.
2. Claim the agent lease: `bus claims stake --agent $AGENT "agent://role"`.
3. Spawn with an explicit identity (e.g., via botty or agent-loop.sh).
4. Announce with `-L spawn-ack`.

### Reviews

- Use `crit` to create reviews and `@<project>-<role>` mentions to spawn reviewers.
- To request a security review:
  1. `crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT`
  2. `bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request`
  (The @mention in the bus message triggers the auto-spawn hook)
- Reviewer agents loop until no pending reviews remain (see review-loop doc).

### Cross-Project Feedback

When you encounter issues with tools from other projects:

1. Query the `#projects` registry: `bus inbox --agent $AGENT --channels projects --all`
2. Find the project entry (format: `project:<name> repo:<path> lead:<agent> tools:<tool1>,<tool2>`)
3. Navigate to the repo, create beads with `br create`
4. Post to the project channel: `bus send <project> "Filed beads: <ids>. <summary> @<lead>" -L feedback`

See [report-issue.md](.agents/botbox/report-issue.md) for details.

### Stack Reference

| Tool | Purpose | Key commands |
|------|---------|-------------|
| bus | Communication, claims, presence | `send`, `inbox`, `claim`, `release`, `agents` |
| maw | Isolated jj workspaces | `ws create`, `ws merge`, `ws destroy` |
| br/bv | Work tracking + triage | `ready`, `create`, `close`, `--robot-next` |
| crit | Code review | `review`, `comment`, `lgtm`, `block` |
| botty | Agent runtime | `spawn`, `kill`, `tail`, `snapshot` |

### Loop Scripts

Scripts in `.agents/botbox/scripts/` automate agent loops:

| Script | Purpose |
|--------|---------|
| `agent-loop.mjs` | Worker: sequential triage-start-work-finish |
| `dev-loop.mjs` | Lead dev: triage, parallel dispatch, merge |
| `reviewer-loop.mjs` | Reviewer: review loop until queue empty |

Usage: `bun .agents/botbox/scripts/<script>.mjs <project-name> [agent-name]`
<!-- botbox:managed-end -->
