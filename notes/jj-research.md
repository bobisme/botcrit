# jj CLI Output Formats Research

Research for botcrit's JJ integration layer.

**jj version tested:** 0.37.0

## Key Findings

### No Native JSON Mode

jj does not have a global `--json` flag. However, it has a powerful **template system** with a `json()` function that can serialize values. Templates are specified via `-T`/`--template`.

### Template Syntax Basics

- `++` concatenates templates
- `"\n"` adds newlines
- `.short()` truncates IDs to 12 chars
- Methods are called with `.method()` or `.method(arg)`
- Built-in keywords available for each context (commit, diff entry, etc.)

---

## Operation 1: Get Current Change ID

The change_id is jj's stable identifier that survives rewrites (unlike commit hashes).

```bash
# Full change_id (32 chars)
jj log -r @ --no-graph -T 'change_id'
# Output: nqtwsmnqroyrvnwrnzrouvyuyxlxsorl

# Short change_id (12 chars) - recommended for display
jj log -r @ --no-graph -T 'change_id.short()'
# Output: nqtwsmnqroyr

# With newline (useful for shell capture)
jj log -r @ --no-graph -T 'change_id ++ "\n"'
```

**Recommended for botcrit:**
```bash
jj log -r @ --no-graph -T 'change_id'
```

---

## Operation 2: Get Current Commit Hash

The commit_id is the Git-compatible SHA-1 hash.

```bash
# Full commit hash (40 chars)
jj log -r @ --no-graph -T 'commit_id'
# Output: df40108eeddc9c7549def91e9f76dabb5f436109

# Short commit hash (12 chars)
jj log -r @ --no-graph -T 'commit_id.short()'
# Output: df40108eeddc
```

**Recommended for botcrit:**
```bash
jj log -r @ --no-graph -T 'commit_id'
```

---

## Operation 3: Get File Diffs Between Commits

### Git-Format Diff (for parsing)

```bash
# Diff between two specific revisions
jj diff --from <rev1> --to <rev2> --git

# Diff of current working copy vs parent
jj diff -r @ --git

# Diff of specific file
jj diff --from <rev1> --to <rev2> --git path/to/file.rs

# Diff from root (all changes)
jj diff --from 'root()' --to @ --git
```

**Important:** Quote revsets containing parentheses to avoid shell interpretation.

### Summary Format (for listing changed files)

```bash
# M/A/D prefix with paths
jj diff -r @ --summary
# Output:
# M .beads/issues.jsonl
# A new-file.txt

# Just file names
jj diff -r @ --name-only
# Output:
# .beads/issues.jsonl
# new-file.txt
```

### Templated Diff Output

When using `-T` with `jj diff`, you get access to `TreeDiffEntry` fields:

```bash
# Custom format: path and status
jj diff -r @ -T 'path ++ " " ++ status ++ "\n"'
# Output:
# .beads/issues.jsonl modified
# new-file.txt added

# Status codes: modified, added, removed, copied, renamed
# Single-char codes available via status_char: M, A, D, C, R
jj diff -r @ -T 'status_char ++ " " ++ path ++ "\n"'
```

**Note:** `-T` cannot be combined with `--summary`, `--stat`, `--types`, or `--name-only`.

**Recommended for botcrit:**
- Use `--git` format for actual diff content (standard, parseable)
- Use `--name-only` or `-T 'path ++ "\n"'` for file lists

---

## Operation 4: Check if File Exists at Commit

```bash
# List files matching a path at a revision
jj file list -r <rev> path/to/file.rs

# If file exists: outputs the path
# If file doesn't exist: outputs nothing + warning to stderr
```

**Gotcha:** Exit code is 0 even when file doesn't exist! Must check stdout.

```bash
# Reliable existence check
output=$(jj file list -r @ src/main.rs 2>/dev/null)
if [ -n "$output" ]; then
    echo "File exists"
else
    echo "File does not exist"
fi
```

**Recommended for botcrit:**
```bash
jj file list -r <rev> <path>
```
Parse stdout: non-empty = exists, empty = doesn't exist.

---

## Operation 5: Get File Contents at Commit

```bash
# Print file contents
jj file show -r <rev> path/to/file.rs

# Multiple files (recursive for directories)
jj file show -r <rev> src/
```

**Recommended for botcrit:**
```bash
jj file show -r <rev> <path>
```

---

## Additional Useful Templates

### Multiple Fields

```bash
# Get multiple values in one call
jj log -r @ --no-graph -T 'change_id ++ " " ++ commit_id ++ "\n"'

# Get parent information
jj log -r @ --no-graph -T 'parents.map(|p| p.change_id()).join(",")'
```

### Commit Metadata

```bash
# Description
jj log -r @ --no-graph -T 'description'

# Author info
jj log -r @ --no-graph -T 'author.email()'
jj log -r @ --no-graph -T 'author.timestamp()'
```

---

## Gotchas and Edge Cases

1. **Shell Quoting**: Revsets with parentheses need quotes: `--from 'root()'`

2. **Exit Codes**: `jj file list` returns 0 even for non-existent files; check stdout instead

3. **No Global JSON**: Must use template system for structured output

4. **Template vs Format Flags**: `-T` is mutually exclusive with `--summary`, `--stat`, etc.

5. **Change ID Stability**: change_id survives rebases/amends; commit_id changes. Use change_id for tracking threads across time.

6. **Working Copy (@)**: The `@` revset always refers to the current working copy commit.

7. **Color Output**: Add `--color=never` when parsing output programmatically to avoid ANSI codes.

---

## Recommended Implementation Strategy

For the `JjRepo` struct in botcrit:

```rust
// Pseudocode - actual implementation in jj.rs

fn get_current_change_id(&self) -> Result<String> {
    // jj log -r @ --no-graph --color=never -T 'change_id'
}

fn get_current_commit(&self) -> Result<String> {
    // jj log -r @ --no-graph --color=never -T 'commit_id'
}

fn diff_files(&self, from: &str, to: &str) -> Result<String> {
    // jj diff --from <from> --to <to> --git --color=never
}

fn file_exists(&self, rev: &str, path: &str) -> Result<bool> {
    // jj file list -r <rev> <path> --color=never
    // Check if stdout is non-empty
}

fn show_file(&self, rev: &str, path: &str) -> Result<String> {
    // jj file show -r <rev> <path>
}

fn changed_files(&self, rev: &str) -> Result<Vec<String>> {
    // jj diff -r <rev> --name-only --color=never
}
```

Always use `--color=never` when capturing output programmatically.
