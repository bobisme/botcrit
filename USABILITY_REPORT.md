# Crit Usability Report

**Date**: 2026-01-25  
**Tester**: Claude (AI agent)  
**Version**: 0.1.0

## Executive Summary

Crit is a functional and well-designed code review tool for jj repositories. It successfully enables multi-agent code review workflows with a clean CLI interface. After running a simulated review with 3 agents (alice, bob, charlie), the tool proved capable and practical for agent-to-agent code review coordination.

**Overall Rating**: 8/10 - Ready for agent use with some improvements needed.

---

## What Works Well

### 1. Clean Command Structure
The CLI follows predictable patterns:
- `crit <resource> <action>` (e.g., `crit reviews create`, `crit threads list`)
- Consistent flags across commands (`--json`, `--author`)
- Good help text and error messages

### 2. Agent Identity System
The `CRIT_AGENT` environment variable works seamlessly:
```bash
CRIT_AGENT=bob crit comments add th-xxx "LGTM"
```
This is exactly what agents need - no config files, just env vars.

### 3. Thread Context Extraction
The `--context N` flag on `threads show` is excellent:
```
> 19 |         "-" => num1 - num2,
> 20 |         "*" => num1 * num2,
  21 |         "/" => num1 / num2,
```
The `>` markers clearly show which lines the thread anchors to.

### 4. TOON Output Format
The default output is readable while being parseable:
```
threads[3]{comment_count,file_path,status,thread_id}:
  2,src/main.rs,resolved,th-jj0m
  3,src/main.rs,resolved,th-34dy
  4,src/main.rs,resolved,th-0udi
```

### 5. JSON Mode for Scripting
Every command supports `--json`, enabling:
```bash
REVIEW_ID=$(crit reviews create --title "X" --json | jq -r '.review_id')
```

### 6. Doctor Command
Health checks with actionable remediation hints are invaluable for debugging.

### 7. Event Sourcing Design
The append-only JSONL log is simple, auditable, and merge-friendly.

---

## Issues Encountered

### Issue 1: Thread Selection Anchors to Wrong Code (Minor)
**Problem**: When viewing a thread, the context shows the code *at the commit where the thread was created*, not the current code.

**Impact**: After Alice fixed the division-by-zero issue, `threads show` still displayed the old code.

**Is this a bug?**: Actually, no - this is arguably correct behavior. The thread was created pointing at specific code, and showing that code helps understand the original context.

**Suggestion**: Add a `--current` flag to show context at HEAD, or show both:
```
Original context (at b1220405):
  21 |         "/" => num1 / num2,

Current context:
  21 |         "/" => {
  22 |             if num2 == 0.0 {
```

### Issue 2: No Way to See Full Thread Conversation Inline (Minor)
**Problem**: `threads show` displays comments as a list, but when reading a thread, you want to see the flow of conversation with context.

**Suggestion**: Add a `--verbose` or `--conversation` mode:
```
th-0udi on src/main.rs:19-20

[bob @ 17:20] Division by zero is not handled!
[charlie @ 17:20] +1 on this
[alice @ 17:21] Fixed! See lines 21-24
[bob @ 17:21] Verified, LGTM

Status: resolved (Fixed by author)
```

### Issue 3: No `reviews merge` Command
**Problem**: Once approved, there's no way to mark a review as merged.

**Workaround**: The event type exists (`ReviewMerged`) but no CLI command exposes it.

**Suggestion**: Add `crit reviews merge <id> --commit <hash>` or auto-detect from jj.

### Issue 4: Drift Detection Doesn't Update Thread Line Numbers
**Problem**: The `status` command shows drift but doesn't update the stored thread data.

**Observed**: After modifying the file, `status` correctly showed threads but drift_status was "unchanged" because we hadn't committed yet (threads compare against commit hashes).

**Note**: This may be working as designed - drift only makes sense between commits.

### Issue 5: No Way to Filter Comments by Author
**Problem**: In a busy thread, you can't filter to see only your own comments or only comments from a specific agent.

**Suggestion**: Add `crit comments list th-xxx --author bob`

### Issue 6: Missing `reviews list` Filters
**Problem**: Can filter by status and author, but not by:
- Reviews I need to review (where I'm a requested reviewer)
- Reviews with unresolved threads
- Reviews older than X days

**Suggestion**: Add `--needs-review`, `--has-unresolved`, `--since` flags.

---

## Usability for AI Agents

### Strengths for Agent Use

1. **Deterministic IDs**: Short, readable IDs like `cr-4in0`, `th-0udi` are easy to reference in messages.

2. **JSON Output**: Agents can reliably parse output:
```bash
# Get all open threads for a review
crit threads list cr-xxx --json | jq '.[] | select(.status == "open")'
```

3. **Stateless Commands**: No interactive prompts, no required config files - just input → output.

4. **Event Log is LLM-Readable**: The `.crit/events.jsonl` can be read directly for full context:
```json
{"ts":"2026-01-25T17:20:29Z","author":"bob","event":"CommentAdded","data":{"body":"Division by zero..."}}
```

5. **Idempotency Support**: The `--request-id` flag prevents duplicate comments from retried requests.

### Weaknesses for Agent Use

1. **No Batch Operations**: Can't create multiple threads or add multiple comments in one command.

2. **No Notification/Watch System**: Agents need to poll `crit status` to detect new activity.

3. **No Rich Formatting**: Comments are plain text only. Agents might want to include code snippets, links, or structured data.

4. **No Thread Linking**: Can't reference another thread like "See th-0udi for related discussion".

---

## Recommendations

### High Priority (for agent use)

1. **Add `--current` flag to `threads show`** to display context at HEAD
2. **Add `reviews merge` command** to complete the review lifecycle
3. **Add `--needs-review` filter** to `reviews list` for finding reviews where the current agent is a requested reviewer

### Medium Priority

4. **Add batch resolve**: `crit threads resolve th-xxx th-yyy th-zzz`
5. **Add conversation view**: `crit threads show th-xxx --conversation`
6. **Add `reviews list --has-unresolved`** filter

### Low Priority (nice to have)

7. **Support markdown in comments** (even if just for rendering)
8. **Add thread cross-references** (e.g., `th-xxx -> th-yyy`)
9. **Add review templates** for common review types

---

## Conclusion

Crit is ready for agent use. The core workflow - creating reviews, adding threads, commenting, and resolving - works smoothly. The tool's design choices (event sourcing, JSON output, env-var identity) are well-suited to multi-agent environments.

The issues found are minor usability improvements rather than blockers. An agent can successfully:
- Create and manage reviews
- Leave contextual comments on specific code lines
- Respond to other agents' feedback
- Track review progress via `status`
- Complete reviews via `approve`

I would recommend this tool for agent-to-agent code review workflows, with the caveat that some manual `jj` commands are still needed for the actual merge step.
