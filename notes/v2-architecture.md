# v2 Architecture: Per-Review Event Logs

## Problem Statement

v1 architecture used a single `.crit/events.jsonl` for all reviews. This caused:
1. **Merge conflicts** - Concurrent reviews in different workspaces all write to same file
2. **Data loss** - jj workspace operations (squash, rebase, merge) can replace file with older version
3. **maw auto-resolve issues** - If `.crit/**` in auto_resolve, discards ALL workspace events

Root cause: Unit of isolation (review) didn't match unit of storage (single file).

## v2 Solution

Each review gets its own event log:

```
.crit/
  version           # "2" - data format version
  reviews/
    cr-abc/
      events.jsonl  # only events for cr-abc
    cr-def/
      events.jsonl  # only events for cr-def
  index.db          # projection rebuilt from all review logs (gitignored)
```

## Key Design Decisions

1. **Reviews travel with code** - This is a HARD REQUIREMENT. Review data must be checked into repo. No external storage.

2. **Per-review isolation** - Different reviews = different files = no conflicts

3. **Timestamp-based sync** - v2 uses `last_sync_ts` instead of line numbers. Simpler since each review log is isolated.

4. **Graceful migration** - `crit migrate` converts v1 → v2, backs up old file

5. **Version enforcement** - Commands fail with clear message if v1 detected, prompting migration

## Implementation Status

### ✅ Completed

| Component | File | Key Functions |
|-----------|------|---------------|
| Version detection | `src/version.rs` | `detect_version()`, `require_v2()`, `write_version_file()` |
| Per-review logs | `src/log/mod.rs` | `ReviewLog`, `list_review_ids()`, `read_all_reviews()`, `open_or_create_review()` |
| Migrate command | `src/cli/commands/migrate.rs` | `run_migrate()` with --dry-run, --backup |
| v2 projection sync | `src/projection/mod.rs` | `sync_from_review_logs()`, `rebuild_from_review_logs()` |
| Init creates v2 | `src/cli/commands/init.rs` | Creates version file + reviews dir |
| Centralized helpers | `src/cli/commands/helpers.rs` | `open_and_sync()` (version-aware), `ensure_initialized()` |
| Command write paths | `reviews.rs`, `threads.rs`, `comments.rs` | All write to per-review logs via `open_or_create_review()` |
| Version enforcement | All commands | `require_v2()` called in `open_and_sync()` |

### ❌ Remaining

1. **Edge case testing** - Aggressive /tmp testing for:
   - Concurrent writes to same review
   - Concurrent writes to different reviews
   - Partial migration (crash mid-way)
   - Corrupt single review log (graceful handling)

## Key Code Paths

### Version Detection
```rust
// src/version.rs
pub fn detect_version(crit_root: &Path) -> Result<Option<DataVersion>>
// Returns: Some(V1), Some(V2), or None (not initialized)

pub fn require_v2(crit_root: &Path) -> Result<()>
// Fails with migration instructions if V1
```

### Writing Events (v2)
```rust
// src/log/mod.rs
let log = ReviewLog::new(crit_root, review_id);
log.append(&event)?;
// Writes to: .crit/reviews/{review_id}/events.jsonl
```

### Reading Events (v2)
```rust
// src/log/mod.rs
let all_events = read_all_reviews(crit_root)?;
// Reads all reviews, sorted by timestamp
```

### Projection Sync (v2)
```rust
// src/projection/mod.rs
sync_from_review_logs(&db, crit_root)?;
// Timestamp-based incremental sync

rebuild_from_review_logs(&db, crit_root)?;
// Full rebuild from all review logs
```

### Migration
```rust
// src/cli/commands/migrate.rs
run_migrate(crit_root, dry_run, backup, format)?;
// Groups v1 events by review_id, writes to per-review logs
// Backs up events.jsonl → events.jsonl.v1.backup
// Writes version file
// Deletes index.db to force rebuild
```

## Beads

- **bd-37x**: Epic - Per-review event log architecture
- **bd-3rv**: Version detection (implemented)
- **bd-3a4**: Per-review log read/write (implemented)
- **bd-15h**: Projection scan per-review logs (implemented, needs command integration)
- **bd-146**: Migration tool (implemented)
- **bd-50e**: Workspace isolation for writes (remaining - commands still write to crit_root)

## Migration Path for Users

```bash
# Check current version
crit doctor  # Will show version info

# Dry run to see what would migrate
crit migrate --dry-run

# Actually migrate
crit migrate

# Verify
ls .crit/reviews/    # Should see cr-xxx directories
cat .crit/version    # Should be "2"
```

## Testing Notes

All 171 tests pass. New tests added:
- `src/version.rs`: 12 tests for version detection
- `src/log/mod.rs`: 10 tests for ReviewLog, list_review_ids, read_all_reviews
- `src/cli/commands/migrate.rs`: 5 tests for migration scenarios
- `src/cli/commands/init.rs`: 6 tests including v1/v2 detection
- `src/cli/commands/helpers.rs`: 5 tests for centralized open_and_sync

## Related Beads (maw)

- **bd-1cl** (maw): Was blocked on this fix. Closed as resolved since detection/recovery is the solution within the "reviews travel with code" constraint.
