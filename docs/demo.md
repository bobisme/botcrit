# Demo Project

Generate a realistic crit demo with `scripts/generate-demo.sh`. It creates a
jj repository in `/tmp` with sample Rust source files, initializes crit, and
exercises reviews, threads, comments, replies, votes, and lifecycle transitions.

## Generate

```bash
./scripts/generate-demo.sh
# Prints the demo directory path to stdout
```

Or specify a custom path:

```bash
./scripts/generate-demo.sh /path/to/demo
```

## What It Creates

- **6 reviews** across 4 agents (swift-falcon, bold-tiger, quiet-owl, mystic-pine)
- **Review 1** (open): Auth refactor — 5 threads, 2 replies, 1 resolved, 1 LGTM + 1 block
- **Review 2** (merged): Config improvements — 1 thread (resolved), approved and merged
- **Review 3** (abandoned): Server TCP listener — 1 thread, abandoned in favor of async
- **Review 4** (approved): API handler — 2 threads, 1 LGTM (api-feature workspace)
- **Review 5** (open): Logging module — 1 thread (logging-feature workspace)
- **Review 6** (open, bare): Server connection handler — no threads, no votes (for manual testing)

## Example Output

### `crit reviews list`

```
[3]{author,open_thread_count,review_id,status,thread_count,title}:
  quiet-owl,1,cr-c05a,abandoned,1,"Server: add TCP listener"
  bold-tiger,0,cr-89bt,merged,1,"Config: add Default impl and env var overrides"
  swift-falcon,4,cr-cccn,open,5,"Refactor auth: replace unsafe static with RwLock"
```

### `crit review <id>`

Shows the full review with inline code context, threads, comments, and votes:

```
○ cr-cccn · Refactor auth: replace unsafe static with RwLock
  Status: open | Author: swift-falcon | Created: 2026-01-28

  Replaces the unsafe mutable static SESSIONS HashMap with a properly
  synchronized RwLock. Also adds token expiry validation and session revocation.

  Votes:
    ✓ bold-tiger (lgtm): Looks good overall. The unsafe removal is solid.
    ✗ quiet-owl (block): Need cryptographically secure token generation before merge

━━━ src/auth.rs ━━━

  ○ th-rrmx (line 4)
    1 | use std::collections::HashMap;
    2 | use std::sync::RwLock;
    3 | use std::time::{SystemTime, UNIX_EPOCH};
  > 4 |
    5 | pub struct Session;
    ...

    ▸ bold-tiger:
       Nice — removing the unsafe block is a big improvement.

  ○ th-vz22 (line 14)
    ...
    ▸ bold-tiger:
       Should we bound the session map size? In production this could grow
       unbounded if sessions aren't cleaned up.

    ▸ swift-falcon:
       Good point. I'll add a max_sessions config option and a background
       cleanup task.

  ✓ th-rb0k (line 43) [resolved]
    ...
    ▸ quiet-owl:
       fastrand isn't cryptographically secure. For session tokens, use
       rand::OsRng or similar.

    ▸ swift-falcon:
       You're right, I'll switch to rand::OsRng. Good catch.
```

### `crit threads list <id> -v`

```
○ th-rrmx src/auth.rs:4 (open, 1 comment)
    bold-tiger: Nice — removing the unsafe block is a big improvement.
○ th-vz22 src/auth.rs:14 (open, 2 comments)
    bold-tiger: Should we bound the session map size? In production this could grow unbounded...
○ th-hqki src/auth.rs:22 (open, 1 comment)
    quiet-owl: Consider returning a Result instead of unwrap() on the RwLock...
○ th-hw09 src/auth.rs:37 (open, 1 comment)
    bold-tiger: The revoke function looks good. Should we also add revoke_all_for_user...
✓ th-rb0k src/auth.rs:43 (resolved, 2 comments)
    quiet-owl: fastrand isn't cryptographically secure. For session tokens, use rand::OsRng...
```

### `crit inbox` (as swift-falcon, the review author)

```
Inbox for swift-falcon (4 items)

Open feedback on your reviews (4):
  th-hqki · src/auth.rs:22 by quiet-owl (1 comments)
    in cr-cccn (Refactor auth: replace unsafe static with RwLock)
  th-vz22 · src/auth.rs:14 by bold-tiger (2 comments)
    in cr-cccn (Refactor auth: replace unsafe static with RwLock)
  th-hw09 · src/auth.rs:37 by bold-tiger (1 comments)
    in cr-cccn (Refactor auth: replace unsafe static with RwLock)
  th-rrmx · src/auth.rs:4 by bold-tiger (1 comments)
    in cr-cccn (Refactor auth: replace unsafe static with RwLock)
```

### `crit inbox` (as bold-tiger, a reviewer)

```
Inbox for bold-tiger (1 items)

Threads with new responses (1):
  th-vz22 · src/auth.rs:14 (+1 new)
    in cr-cccn (Refactor auth: replace unsafe static with RwLock)
```

### `crit doctor`

```
checks[5]{message,name,status}:
  "jj is installed: jj 0.37.0",jj_installed,pass
  Current directory is a jj repository,jj_repo,pass
  ".crit/ exists, events.jsonl present, index.db present",crit_initialized,pass
  events.jsonl is valid (31 events),events_parseable,pass
  "index.db is in sync (3 reviews, 0 events)",index_sync,pass
healthy: true
```

## Features Exercised

| Feature | Where |
|---------|-------|
| `crit init` | Setup |
| `reviews create` | Reviews 1, 2, 3 |
| `reviews request` | Reviews 1, 2 |
| `comment` (auto-thread) | All reviews |
| `reply` | Reviews 1, 2 |
| `threads resolve` | Reviews 1, 2 |
| `lgtm` | Reviews 1, 2 |
| `block` | Review 1 |
| `reviews approve` | Review 2 |
| `reviews mark-merged` | Review 2 |
| `reviews abandon` | Review 3 |
