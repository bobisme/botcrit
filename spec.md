# Botcrit: Agent-Centric Distributed Code Review Tool

**Version:** 0.1.0 (Draft)  
**Stack:** Rust, Jujutsu (jj), SQLite, JSONL  
**Philosophy:** "Beads" Architecture (Append-Only Event Log)

## 1. Executive Summary

botcrit is a CLI-first code review tool designed specifically for AI Agents working in a distributed, asynchronous environment. Unlike traditional tools that rely on central servers (GitHub/GitLab), botcrit operates locally on the filesystem using an append-only event log (`events.jsonl`).

It leverages Jujutsu (jj) to solve the "rebase problem" inherent in agent workflows. By anchoring reviews to stable `change_id`s while tracking comments against specific `commit_hash` snapshots, botcrit allows agents to review code that is actively being rewritten without losing context.

## 2. Core Architecture

### 2.1 The Data Flow

The system follows an Event Sourcing pattern with a CQRS (Command Query Responsibility Segregation) split.

- **Write Path (Command):** Agents append immutable events to `.crit/events.jsonl`. This file is the single source of truth.
- **Read Path (Query):** A "Projection" process reads the log and updates a local SQLite database (`.crit/index.db`). This allows for instant querying of complex states (e.g., "Show me all unresolved threads for this file").

### 2.2 Storage Model

| Component   | Description                              |
|-------------|------------------------------------------|
| Directory   | `.crit/` at repository root              |
| Log         | `.crit/events.jsonl` (Human-readable, append-only) |
| Index       | `.crit/index.db` (Ephemeral, rebuildable cache)  |
| Concurrency | Writes to `events.jsonl` are guarded by advisory file locks (`fs2` crate) to ensure atomic appends from multiple concurrent agents |

### 2.3 Agent Identity

The `author` field on events is determined by (in priority order):

1. `--author` flag (explicit override)
2. `CRIT_AGENT` environment variable
3. `BOTBUS_AGENT` environment variable (interop with BotBus)
4. `USER` environment variable (fallback for human users)

### 2.4 The jj Integration Strategy

Git-based tools fail when commits are amended. botcrit succeeds by distinguishing between:

- **The Review Target:** Anchored to a jj Change ID (persistent across rebases/amends).
- **The Comment Anchor:** Anchored to a specific Commit Hash (snapshot in time).

## 3. Data Model

The event log consists of strictly typed events serialized as JSON Lines. Each event has a common envelope:

```json
{"ts": "2024-01-20T10:00:00Z", "author": "alice_agent", "event": "ReviewCreated", "data": {...}}
```

### Event Types

| Event | Key Fields | Description |
|-------|------------|-------------|
| `ReviewCreated` | `review_id`, `jj_change_id`, `initial_commit`, `title`, `description?` | New review opened |
| `ReviewersRequested` | `review_id`, `reviewers[]` | Reviewers assigned |
| `ReviewApproved` | `review_id` | Review approved by author |
| `ReviewMerged` | `review_id`, `final_commit` | Review merged |
| `ReviewAbandoned` | `review_id`, `reason?` | Review closed without merge |
| `ThreadCreated` | `thread_id`, `review_id`, `file_path`, `selection`, `commit_hash` | Comment thread opened on specific lines |
| `CommentAdded` | `comment_id`, `thread_id`, `body`, `request_id?` | Comment added to thread |
| `ThreadResolved` | `thread_id`, `reason?` | Thread marked resolved |
| `ThreadReopened` | `thread_id`, `reason?` | Thread reopened |

### Selection Types

- `Line(n)` - Single line
- `Range(start, end)` - Inclusive line range

## 4. Drift Detection (The "Magic")

Since agents review specific snapshots while the code evolves, botcrit must calculate where comments "live" in the current version.

### Algorithm

1. **Inputs:** `Thread.original_line`, `Thread.original_commit`, `Current.commit`
2. **Diff:** Execute `jj diff --from <original> --to <current> <file_path>`
3. **Map:** Parse diff hunks (using `unidiff`)
   - If lines are inserted before the comment: `current_line += count`
   - If lines are deleted before the comment: `current_line -= count`
   - If the comment line itself is modified/deleted: Status = `Detached` (or `Outdated`)
4. **Output:** The CLI presents the calculated current line number, not the historical one.

## 5. Agent Interface Protocols

Agents are token-sensitive and lack visual intuition. The interface is optimized for them.

### 5.1 Output Format: TOON (default) or JSON

By default, `crit` outputs [TOON](https://toonformat.dev/) (Token-Oriented Object Notation) - a compact, human-readable format optimized for LLM token efficiency. Use `--json` for machine-parseable JSON output.

**Example:** `crit threads show th-99a`

```yaml
thread:
  id: th-99a
  file: src/parser.rs
  original_line: 42
  current_line: 45
  status: Open
  context_hash: 7b3f1a...
context: |
  @@ -42,4 +45,4 @@
   fn parse_buffer(buf: &str) {
  -    // AGENT NOTE: This buffer isn't cleared
       let x = buf.len();
   }
comments[1]{id,author,timestamp,body}:
  c-1,alice_agent,2024-01-20T10:00:00Z,This buffer isn't cleared before reuse. It causes a leak.
```

The `context` field shows the code at the anchored location. Use `--context <lines>` to control how many surrounding lines are shown (default: 3).

**With `--json`:**

```json
{
  "thread": {
    "id": "th-99a",
    "file": "src/parser.rs",
    "original_line": 42,
    "current_line": 45,
    "status": "Open",
    "context_hash": "7b3f1a..."
  },
  "comments": [
    {
      "id": "c-1",
      "author": "alice_agent",
      "timestamp": "2024-01-20T10:00:00Z",
      "body": "This buffer isn't cleared before reuse. It causes a leak."
    }
  ]
}
```

### 5.2 Optimistic Locking (Verify-Before-Write)

Agents can prevent hallucinations by asserting the state of the world when they write.

```bash
crit comments add th-99a "Fix this" --expected-hash 7b3f1a
```

- **Success:** Hash matches `thread.current_commit`.
- **Failure:** Returns `Error: StaleCommit`. The agent must re-read the context.

### 5.3 Idempotency

Agents may retry commands on timeout.

```bash
crit comments add ... --request-id <UUID>
```

Duplicate request IDs are silently ignored (returning success).

## 6. CLI Command Specification

### Setup

```bash
crit init
```
**Action:** Creates `.crit/` directory with empty `events.jsonl`.

### Reviews

```bash
crit reviews create --title "..." [--desc "..."]
```
**Action:** Infers `change_id` from `@`, generates `review_id`.

```bash
crit reviews list [--status open|merged|abandoned] [--author <name>]
crit reviews show <review_id>
crit reviews request <review_id> --reviewers agent_a,agent_b
crit reviews approve <review_id>
crit reviews abandon <review_id> [--reason "..."]
```

### Threads

```bash
crit threads create <review_id> --file <path> --lines <start>[-<end>]
```
**Action:** Locks current commit hash as anchor.

```bash
crit threads list <review_id> [--status open|resolved] [--file <path>]
crit threads show <thread_id> [--context <lines>]
crit threads resolve <thread_id> [--reason "..."]
crit threads reopen <thread_id> [--reason "..."]
```

### Comments

```bash
crit comments add <thread_id> <msg> [--request-id <uuid>] [--expected-hash <hash>]
crit comments list <thread_id>
```

### Querying

```bash
crit status [<review_id>] [--unresolved-only] [--json]
```
**Action:** Without review_id, shows all open reviews. With review_id, shows detailed status including drift detection.

```bash
crit diff <review_id>
```
**Action:** Returns a structured diff of the whole change.

### Batch Operations

```bash
crit threads resolve --all --file <path>    # Resolve all threads in a file
crit threads resolve --all <review_id>      # Resolve all threads in a review
```

### Utilities

```bash
crit doctor
```
**Action:** Health check - verifies jj is installed, repo is jj-managed, `.crit/` exists, `events.jsonl` is valid, `index.db` is in sync. Outputs pass/fail with remediation hints.

```bash
crit agents init
crit agents show
```
**Action:** `init` inserts crit usage instructions into `AGENTS.md` (creates file if needed). `show` outputs the instruction block to stdout. Uses HTML comment markers (`<!-- crit-agent-instructions -->`) for idempotent updates.

## 7. Implementation Roadmap

1. **Core Crate (`botcrit_core`):**
   - Define Event structs with serde
   - Implement `AppendLog` trait with file locking
   - Agent identity resolution

2. **Projection Engine:**
   - SQLite schema setup (`rusqlite`)
   - Sync function (JSONL -> DB)
   - Incremental sync (track last processed line)

3. **JJ Adapter:**
   - Wrapper around `std::process::Command` for `jj log`, `jj diff`
   - Drift calculation logic (parse unified diff)
   - Context extraction for threads

4. **CLI (`crit`):**
   - `clap` definitions
   - Output formatters (TOON default, JSON with `--json`)
   - `crit init` command

5. **TUI (Lower Priority):**
   - `ratatui` terminal interface for interactive review
   - Watch mode: live updates as events are appended
