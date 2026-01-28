<!-- botbus-agent-instructions-v1 -->

## BotBus Agent Coordination

This project uses [BotBus](https://github.com/anomalyco/botbus) for multi-agent coordination. BotBus uses global storage (~/.local/share/botbus/) shared across all projects.

### Quick Start

```bash
# Check what's happening
botbus status              # Overview: agents, channels, claims
botbus history             # Recent messages in #general
botbus agents              # Who's been active

# Communicate
botbus send --agent botcrit-dev botcrit "Starting work on X"
botbus send --agent botcrit-dev botcrit "Done with X, ready for review"
botbus send --agent botcrit-dev @other-agent "Question about Y"

# Coordinate file access (claims use absolute paths internally)
botbus claim --agent botcrit-dev "src/api/**" -m "Working on API routes"
botbus check-claim src/api/routes.rs   # Check before editing
botbus release --agent botcrit-dev --all  # When done
```

### Best Practices

1. **Set BOTBUS_AGENT** at session start - identity is stateless
2. **Run `botbus status`** to see current state before starting work
3. **Claim files** you plan to edit - overlapping claims are denied
4. **Check claims** before editing files outside your claimed area
5. **Send updates** on blockers, questions, or completed work
6. **Release claims** when done - don't hoard files

### Channel Conventions

- `#general` - Default channel for cross-project coordination
- `#project-name` - Project-specific updates (e.g., `#botcrit`, `#backend`)
- `#project-topic` - Sub-topics (e.g., `#botcrit-tui`, `#backend-auth`)
- `@agent-name` - Direct messages for specific coordination

Channel names: lowercase alphanumeric with hyphens (e.g., `my-channel`)

### Message Conventions

Keep messages concise and actionable:

- "Starting work on issue #123: Add foo feature"
- "Blocked: need database credentials to proceed"
- "Question: should auth middleware go in src/api or src/auth?"
- "Done: implemented bar, tests passing"

### Waiting for Replies

```bash
# After sending a DM, wait for reply
botbus send --agent botcrit-dev @other-agent "Can you review this?"
botbus wait --agent botcrit-dev -c @other-agent -t 60  # Wait up to 60s for reply

# Wait for any @mention of you
botbus wait --mention -t 120
```

<!-- end-botbus-agent-instructions -->
<!-- maw-agent-instructions-v1 -->

## Multi-Agent Workflow with MAW

This project uses MAW for coordinating multiple agents via jj workspaces.
Each agent gets an isolated working copy - you can edit files without blocking other agents.

### Workspace Naming

**Your workspace name will be assigned by the coordinator** (human or orchestrating agent).
If you need to create your own, use:

- Lowercase alphanumeric with hyphens: `agent-1`, `feature-auth`, `bugfix-123`
- Check existing workspaces first: `maw ws list`

### Quick Reference

| Task                 | Command                 |
| -------------------- | ----------------------- |
| Create workspace     | `maw ws create <name>`  |
| List workspaces      | `maw ws list`           |
| Check status         | `maw ws status`         |
| Sync stale workspace | `maw ws sync`           |
| Merge all work       | `maw ws merge --all`    |
| Destroy workspace    | `maw ws destroy <name>` |

### Starting Work

```bash
# Check what workspaces exist
maw ws list

# Create your workspace (if not already assigned)
maw ws create <assigned-name>
cd .workspaces/<assigned-name>

# Start working - jj tracks changes automatically
jj describe -m "wip: implementing feature X"
```

### During Work

```bash
# See your changes
jj diff
jj status

# Save your work (describe current commit)
jj describe -m "feat: add feature X"

# Or commit and start fresh
jj commit -m "feat: add feature X"

# See what other agents are doing
maw ws status
```

### Handling Stale Workspace

If you see "working copy is stale", the main repo changed while you were working:

```bash
maw ws sync
```

### Finishing Work

When done, notify the coordinator. They will merge from the main workspace:

```bash
# Coordinator runs from main workspace:
maw ws merge --all --destroy
```

### Resolving Conflicts

jj records conflicts in commits rather than blocking. If you see conflicts:

```bash
jj status  # shows conflicted files
# Edit the files to resolve (remove conflict markers)
jj describe -m "resolve: merge conflicts"
```

<!-- end-maw-agent-instructions -->

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

<!-- br-agent-instructions-v1 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`/`bd`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

### Essential Commands

```bash
# View ready issues (unblocked, not deferred)
br ready              # or: bd ready

# List and search
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br search "keyword"   # Full-text search

# Create and update
br create --title="..." --description="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once

# Sync with git
br sync --flush-only  # Export DB to JSONL
br sync --status      # Check sync status
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Always run `br sync --flush-only` at session end

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads changes to JSONL
git commit -m "..."     # Commit everything
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always sync before ending session

<!-- end-br-agent-instructions -->

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
cargo test

# 2. Bump version in Cargo.toml
#    Edit: version = "X.Y.Z"

# 3. Commit the release
jj commit -m "chore: bump version to X.Y.Z

Co-Authored-By: Claude <noreply@anthropic.com>"

# 4. Update main bookmark and push
jj bookmark set main -r @-
jj git push

# 5. Install locally
just install

# 6. Verify
crit --version

# 7. Announce on botbus
botbus --agent botcrit-dev send botcrit "crit vX.Y.Z released - [summary of changes]"
```

### Quick Reference

| Stage    | Commands                                      |
| -------- | --------------------------------------------- |
| Test     | `cargo test`                                  |
| Bump     | Edit `Cargo.toml` version                     |
| Commit   | `jj commit -m "chore: bump version to X.Y.Z"` |
| Push     | `jj bookmark set main -r @- && jj git push`   |
| Install  | `just install`                                |
| Announce | `botbus send botcrit "crit vX.Y.Z - ..."`     |

