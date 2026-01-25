# Crit Testing Guide

This document provides testing scenarios and shell scripts for validating crit functionality.

## Quick Setup

```bash
# Build crit
cargo build

# Create test environment
export CRIT=/path/to/target/debug/crit
cd /tmp && rm -rf crit-test && mkdir crit-test && cd crit-test
jj git init
$CRIT init
$CRIT doctor
```

## Simulation Test Script

Save as `test-review-flow.sh` and run from a clean directory:

```bash
#!/bin/bash
set -e

CRIT="${CRIT:-crit}"

echo "=== Setting up test repository ==="
rm -rf /tmp/crit-test
mkdir /tmp/crit-test && cd /tmp/crit-test
jj git init
$CRIT init

echo "=== Creating test file ==="
mkdir -p src
cat > src/main.rs << 'EOF'
//! A simple calculator CLI.

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 4 {
        eprintln!("Usage: calc <num1> <op> <num2>");
        std::process::exit(1);
    }
    
    let num1: f64 = args[1].parse().expect("Invalid first number");
    let op = &args[2];
    let num2: f64 = args[3].parse().expect("Invalid second number");
    
    let result = match op.as_str() {
        "+" => num1 + num2,
        "-" => num1 - num2,
        "*" => num1 * num2,
        "/" => num1 / num2,
        _ => {
            eprintln!("Unknown operator: {}", op);
            std::process::exit(1);
        }
    };
    
    println!("{}", result);
}
EOF

jj describe -m "feat: add calculator CLI"

echo "=== Alice creates review ==="
REVIEW_ID=$(CRIT_AGENT=alice $CRIT reviews create --title "Add calculator CLI" --json | jq -r '.review_id')
echo "Created review: $REVIEW_ID"

CRIT_AGENT=alice $CRIT reviews request $REVIEW_ID --reviewers bob,charlie

echo "=== Bob reviews ==="
THREAD1=$(CRIT_AGENT=bob $CRIT threads create $REVIEW_ID --file src/main.rs --lines 21 --json | jq -r '.thread_id')
CRIT_AGENT=bob $CRIT comments add $THREAD1 "Division by zero is not handled!"

THREAD2=$(CRIT_AGENT=bob $CRIT threads create $REVIEW_ID --file src/main.rs --lines 8-11 --json | jq -r '.thread_id')
CRIT_AGENT=bob $CRIT comments add $THREAD2 "Consider using Result instead of process::exit"

echo "=== Charlie reviews ==="
THREAD3=$(CRIT_AGENT=charlie $CRIT threads create $REVIEW_ID --file src/main.rs --lines 1 --json | jq -r '.thread_id')
CRIT_AGENT=charlie $CRIT comments add $THREAD3 "nit: Add usage examples to docs"
CRIT_AGENT=charlie $CRIT comments add $THREAD1 "+1 on division by zero"

echo "=== Status check ==="
$CRIT status

echo "=== Alice responds ==="
# Resolve the nit
CRIT_AGENT=alice $CRIT comments add $THREAD3 "Will add in follow-up"
CRIT_AGENT=alice $CRIT threads resolve $THREAD3 --reason "Will address later"

# Push back on Result type
CRIT_AGENT=alice $CRIT comments add $THREAD2 "Disagree - process::exit is fine for CLI"
CRIT_AGENT=bob $CRIT comments add $THREAD2 "Fair point, withdrawing"
CRIT_AGENT=bob $CRIT threads resolve $THREAD2 --reason "Declined by author"

# Fix division by zero (update the file)
cat > src/main.rs << 'EOF'
//! A simple calculator CLI.

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 4 {
        eprintln!("Usage: calc <num1> <op> <num2>");
        std::process::exit(1);
    }
    
    let num1: f64 = args[1].parse().expect("Invalid first number");
    let op = &args[2];
    let num2: f64 = args[3].parse().expect("Invalid second number");
    
    let result = match op.as_str() {
        "+" => num1 + num2,
        "-" => num1 - num2,
        "*" => num1 * num2,
        "/" => {
            if num2 == 0.0 {
                eprintln!("Error: Division by zero");
                std::process::exit(1);
            }
            num1 / num2
        }
        _ => {
            eprintln!("Unknown operator: {}", op);
            std::process::exit(1);
        }
    };
    
    println!("{}", result);
}
EOF

CRIT_AGENT=alice $CRIT comments add $THREAD1 "Fixed! Added div-by-zero check"
CRIT_AGENT=bob $CRIT comments add $THREAD1 "Verified, LGTM"
CRIT_AGENT=bob $CRIT threads resolve $THREAD1 --reason "Fixed"

echo "=== Final status ==="
$CRIT status
$CRIT reviews show $REVIEW_ID

echo "=== Bob approves ==="
CRIT_AGENT=bob $CRIT reviews approve $REVIEW_ID

echo "=== Final review state ==="
$CRIT reviews show $REVIEW_ID

echo "=== Test complete ==="
```

## Individual Test Cases

### Test 1: Basic Review Lifecycle

```bash
CRIT_AGENT=alice $CRIT reviews create --title "Test review"
$CRIT reviews list
CRIT_AGENT=bob $CRIT reviews approve cr-xxx
$CRIT reviews show cr-xxx
```

### Test 2: Thread Creation and Comments

```bash
# Create thread on single line
CRIT_AGENT=bob $CRIT threads create cr-xxx --file src/main.rs --lines 10

# Create thread on line range
CRIT_AGENT=bob $CRIT threads create cr-xxx --file src/main.rs --lines 10-20

# Add comments
CRIT_AGENT=bob $CRIT comments add th-xxx "This needs work"
CRIT_AGENT=alice $CRIT comments add th-xxx "Fixed!"

# List and show
$CRIT threads list cr-xxx
$CRIT threads show th-xxx --context 3
```

### Test 3: Thread Resolution Workflows

```bash
# Resolve with reason
CRIT_AGENT=bob $CRIT threads resolve th-xxx --reason "Fixed by author"

# Batch resolve all open threads
$CRIT threads resolve --all --reason "Batch close"

# Reopen a thread
CRIT_AGENT=charlie $CRIT threads reopen th-xxx --reason "Actually not fixed"
```

### Test 4: Status and Drift Detection

```bash
# Check overall status
$CRIT status

# Check specific review
$CRIT status cr-xxx

# Only show unresolved threads
$CRIT status --unresolved-only
```

### Test 5: JSON Output for Scripting

```bash
# All commands support --json
$CRIT reviews list --json
$CRIT threads show th-xxx --json
$CRIT status --json | jq '.[] | select(.open_threads > 0)'
```

### Test 6: Doctor Health Check

```bash
# Should pass in initialized repo
$CRIT doctor

# Should fail outside jj repo
cd /tmp && $CRIT doctor
```

### Test 7: Review Abandonment

```bash
CRIT_AGENT=alice $CRIT reviews create --title "Will abandon"
CRIT_AGENT=alice $CRIT reviews abandon cr-xxx --reason "Changed approach"
$CRIT reviews list --status abandoned
```

## Validation Checklist

After running tests, verify:

- [ ] `crit doctor` shows all checks passing
- [ ] `.crit/events.jsonl` contains valid JSON lines
- [ ] Thread context extraction shows correct code
- [ ] Drift detection works when code changes
- [ ] Multiple agents can interact on same review
- [ ] Thread resolve/reopen lifecycle works
- [ ] JSON output is valid and parseable
- [ ] TOON output is readable

## Common Issues

### "Not a crit repository"
Run `crit init` first.

### "Review not found"
Verify review ID with `crit reviews list`.

### "Thread not found"  
Verify thread ID with `crit threads list <review_id>`.

### Context shows old code
This is correct! Thread context shows code at the commit where the thread was created.
Use `crit status` to see drift detection for how lines moved.

### Agent identity
Set `CRIT_AGENT` environment variable, or use `--author` flag.
Falls back to `BOTBUS_AGENT` then `USER`.
