#!/usr/bin/env bash
# Generate a demo project with realistic crit data for screenshots and docs.
#
# Usage:
#   ./scripts/generate-demo.sh          # Creates demo in /tmp/crit-demo-XXXXXX
#   ./scripts/generate-demo.sh /path    # Creates demo at custom path
#
# The demo project is a fake jj repo with multiple reviews, threads,
# comments, votes, and resolved threads — exercising most crit features.

set -euo pipefail

cd "$(dirname "$0")/.."

DEMO_DIR="${1:-$(mktemp -d /tmp/crit-demo-XXXXXX)}"
CRIT="$(pwd)/target/release/crit"

# Build release binary if needed
if [[ ! -x "$CRIT" ]]; then
	echo "Building crit release binary..." >&2
	cargo build --release --quiet
fi

# Clean slate
if [[ -d "$DEMO_DIR" ]] && [[ -d "$DEMO_DIR/.jj" ]]; then
	echo "Removing existing demo at $DEMO_DIR..." >&2
	rm -rf "$DEMO_DIR"
	mkdir -p "$DEMO_DIR"
fi

cd "$DEMO_DIR"

# ============================================================================
# Set up a jj repository with sample source files
# ============================================================================

jj git init --quiet 2>/dev/null
jj config set --repo user.name "Demo User" 2>/dev/null
jj config set --repo user.email "demo@example.com" 2>/dev/null

# Create realistic source files
mkdir -p src

cat > src/main.rs << 'RUST'
use std::env;

mod auth;
mod config;
mod server;

fn main() {
    let config = config::load();
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());

    println!("Starting server on {}", addr);
    server::run(&addr, &config);
}
RUST

cat > src/auth.rs << 'RUST'
use std::collections::HashMap;

pub struct Session {
    pub user_id: String,
    pub token: String,
    pub expires_at: u64,
}

static mut SESSIONS: Option<HashMap<String, Session>> = None;

pub fn validate_token(token: &str) -> Option<&Session> {
    unsafe {
        SESSIONS.as_ref()?.get(token)
    }
}

pub fn create_session(user_id: &str) -> Session {
    let token = format!("tok_{}", rand_hex(32));
    Session {
        user_id: user_id.to_string(),
        token,
        expires_at: now() + 3600,
    }
}

fn rand_hex(_len: usize) -> String {
    "abcdef1234567890".to_string()
}

fn now() -> u64 {
    0
}
RUST

cat > src/config.rs << 'RUST'
pub struct Config {
    pub database_url: String,
    pub max_connections: u32,
    pub log_level: String,
}

pub fn load() -> Config {
    Config {
        database_url: std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://localhost/app".into()),
        max_connections: 10,
        log_level: "info".into(),
    }
}
RUST

cat > src/server.rs << 'RUST'
use crate::config::Config;

pub fn run(addr: &str, config: &Config) {
    println!("Connecting to database: {}", config.database_url);
    println!("Max connections: {}", config.max_connections);
    println!("Listening on {}", addr);
    // Main loop would go here
}
RUST

cat > Cargo.toml << 'TOML'
[package]
name = "demo-app"
version = "0.1.0"
edition = "2021"
TOML

cat > README.md << 'MD'
# demo-app

A sample web server for demonstrating crit code review.
MD

# Commit the initial codebase
jj commit -m "feat: initial project structure

Add main server, auth module, config, and server setup." 2>/dev/null

# ============================================================================
# Initialize crit
# ============================================================================

"$CRIT" --agent setup-bot init >/dev/null 2>&1

echo "Crit initialized." >&2

# ============================================================================
# Helpers
# ============================================================================

crit_as() {
	local agent="$1"
	shift
	"$CRIT" --agent "$agent" "$@"
}

# Extract a field from JSON output (requires jq)
extract_id() {
	local field="$1"
	jq -r ".$field"
}

# ============================================================================
# Review 1: Auth refactor (open, active discussion)
# ============================================================================

# Make auth changes on a new commit
# Also make a tiny change to main.rs (for testing comments outside changed range)
cat > src/main.rs << 'RUST'
use std::env;

mod auth;
mod config;
mod server;

fn main() {
    let config = config::load();
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());

    println!("Starting server on {}", addr);

    // Initialize auth module
    auth::init_sessions();

    server::run(&addr, &config);
}
RUST

cat > src/auth.rs << 'RUST'
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session {
    pub user_id: String,
    pub token: String,
    pub expires_at: u64,
}

lazy_static::lazy_static! {
    static ref SESSIONS: RwLock<HashMap<String, Session>> = RwLock::new(HashMap::new());
}

pub fn init_sessions() {
    // Force initialization of lazy_static
    let _ = &*SESSIONS;
}

pub fn validate_token(token: &str) -> bool {
    let sessions = SESSIONS.read().unwrap();
    match sessions.get(token) {
        Some(session) => session.expires_at > now(),
        None => false,
    }
}

pub fn create_session(user_id: &str) -> Session {
    let token = format!("tok_{}", rand_hex(32));
    let session = Session {
        user_id: user_id.to_string(),
        token: token.clone(),
        expires_at: now() + 3600,
    };
    SESSIONS.write().unwrap().insert(token.clone(), Session {
        user_id: user_id.to_string(),
        token: token.clone(),
        expires_at: session.expires_at,
    });
    session
}

pub fn revoke_session(token: &str) -> bool {
    SESSIONS.write().unwrap().remove(token).is_some()
}

fn rand_hex(len: usize) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        write!(s, "{:x}", fastrand::u8(..)).unwrap();
    }
    s
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
RUST

jj describe -m "refactor(auth): replace unsafe static with RwLock

Replace unsafe mutable static SESSIONS with lazy_static RwLock.
Add session expiry checking and revocation support." 2>/dev/null

# Create the review while changes are in @ (so change ID captures auth.rs)
R1=$(crit_as "swift-falcon" --json reviews create \
	--title "Refactor auth: replace unsafe static with RwLock" \
	--desc "Replaces the unsafe mutable static SESSIONS HashMap with a properly synchronized RwLock. Also adds token expiry validation and session revocation." \
	2>/dev/null | extract_id review_id)

jj new 2>/dev/null

echo "Review 1: $R1" >&2

# Request reviewers
crit_as "swift-falcon" reviews request "$R1" --reviewers "bold-tiger,quiet-owl" >/dev/null 2>&1

# Bold-tiger reviews and leaves comments
crit_as "bold-tiger" comment "$R1" --file src/auth.rs --line 4 \
	"Nice — removing the unsafe block is a big improvement." >/dev/null 2>&1

T_SIZE=$(crit_as "bold-tiger" --json comment "$R1" --file src/auth.rs --line 14 \
	"Should we bound the session map size? In production this could grow unbounded if sessions aren't cleaned up." \
	2>/dev/null | extract_id thread_id)

crit_as "bold-tiger" comment "$R1" --file src/auth.rs --line 37 \
	"The revoke function looks good. Should we also add a revoke_all_for_user for account lockout scenarios?" >/dev/null 2>&1

# Comment on main.rs at an UNCHANGED line (line 3, the mod auth declaration)
crit_as "bold-tiger" comment "$R1" --file src/main.rs --line 3 \
	"Good call adding the init_sessions() call, but should we also add error handling here in case auth module fails to initialize?" >/dev/null 2>&1

# Comment on a file NOT IN THE CHANGE at all (config.rs wasn't touched in Review 1)
crit_as "quiet-owl" comment "$R1" --file src/config.rs --line 6 \
	"Since we're adding session management, should we also add a session_max_size config here?" >/dev/null 2>&1

# Swift-falcon replies to the size concern
crit_as "swift-falcon" reply "$T_SIZE" \
	"Good point. I'll add a max_sessions config option and a background cleanup task." >/dev/null 2>&1

# Quiet-owl also reviews
crit_as "quiet-owl" comment "$R1" --file src/auth.rs --line 22 \
	"Consider returning a Result instead of unwrap() on the RwLock. A poisoned lock would panic the server." >/dev/null 2>&1

T_CRYPTO=$(crit_as "quiet-owl" --json comment "$R1" --file src/auth.rs --line 43 \
	"fastrand isn't cryptographically secure. For session tokens, use rand::OsRng or similar." \
	2>/dev/null | extract_id thread_id)

# Swift-falcon acknowledges the crypto concern
crit_as "swift-falcon" reply "$T_CRYPTO" \
	"You're right, I'll switch to rand::OsRng. Good catch." >/dev/null 2>&1
crit_as "swift-falcon" threads resolve "$T_CRYPTO" \
	--reason "Switched to rand::OsRng in follow-up commit" >/dev/null 2>&1

# Bold-tiger approves
crit_as "bold-tiger" lgtm "$R1" -m "Looks good overall. The unsafe removal is solid." >/dev/null 2>&1

# Quiet-owl blocks pending the crypto fix
crit_as "quiet-owl" block "$R1" -r "Need cryptographically secure token generation before merge" >/dev/null 2>&1

echo "  7 threads (2 on unchanged code), 1 resolved, 1 LGTM, 1 block" >&2

# ============================================================================
# Review 2: Config improvements (approved and merged)
# ============================================================================

cat > src/config.rs << 'RUST'
use std::env;

pub struct Config {
    pub database_url: String,
    pub max_connections: u32,
    pub log_level: String,
    pub bind_addr: String,
    pub session_ttl: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database_url: "postgres://localhost/app".into(),
            max_connections: 10,
            log_level: "info".into(),
            bind_addr: "0.0.0.0:8080".into(),
            session_ttl: 3600,
        }
    }
}

pub fn load() -> Config {
    let mut config = Config::default();

    if let Ok(url) = env::var("DATABASE_URL") {
        config.database_url = url;
    }
    if let Ok(max) = env::var("MAX_CONNECTIONS") {
        config.max_connections = max.parse().unwrap_or(10);
    }
    if let Ok(level) = env::var("LOG_LEVEL") {
        config.log_level = level;
    }
    if let Ok(addr) = env::var("BIND_ADDR") {
        config.bind_addr = addr;
    }
    if let Ok(ttl) = env::var("SESSION_TTL") {
        config.session_ttl = ttl.parse().unwrap_or(3600);
    }

    config
}
RUST

jj describe -m "feat(config): add Default impl and env var overrides

All config fields now have sensible defaults and can be overridden
via environment variables." 2>/dev/null

# Create the review while changes are in @
R2=$(crit_as "bold-tiger" --json reviews create \
	--title "Config: add Default impl and env var overrides" \
	--desc "All config fields now have sensible defaults and can be individually overridden via environment variables." \
	2>/dev/null | extract_id review_id)

jj new 2>/dev/null

echo "Review 2: $R2" >&2

crit_as "bold-tiger" reviews request "$R2" --reviewers "swift-falcon" >/dev/null 2>&1

# Swift-falcon reviews with minor feedback
crit_as "swift-falcon" comment "$R2" --file src/config.rs --line 32 \
	"Consider using a builder pattern instead of mutating fields. It's more idiomatic." >/dev/null 2>&1

# Bold-tiger replies
T_BUILDER=$(crit_as "bold-tiger" --json threads list "$R2" 2>/dev/null | jq -r '.[0].thread_id')
crit_as "bold-tiger" reply "$T_BUILDER" \
	"Fair point, but for a simple config this is more readable. I'll refactor if we add more fields." >/dev/null 2>&1
crit_as "swift-falcon" reply "$T_BUILDER" \
	"That's reasonable. Ship it." >/dev/null 2>&1
crit_as "swift-falcon" threads resolve "$T_BUILDER" \
	--reason "Agreed to defer" >/dev/null 2>&1

# Approve and merge
crit_as "swift-falcon" lgtm "$R2" -m "Clean implementation. Good defaults." >/dev/null 2>&1
crit_as "bold-tiger" reviews approve "$R2" >/dev/null 2>&1
crit_as "bold-tiger" reviews merge "$R2" --self-approve >/dev/null 2>&1

echo "  1 thread (resolved), merged" >&2

# ============================================================================
# Review 3: Server improvements (abandoned)
# ============================================================================

cat > src/server.rs << 'RUST'
use crate::config::Config;
use std::io;
use std::net::TcpListener;

pub fn run(config: &Config) -> io::Result<()> {
    println!("Connecting to {}", config.database_url);
    let listener = TcpListener::bind(&config.bind_addr)?;
    println!("Listening on {}", config.bind_addr);

    for stream in listener.incoming() {
        match stream {
            Ok(_stream) => {
                // handle connection
            }
            Err(e) => eprintln!("Connection failed: {}", e),
        }
    }
    Ok(())
}
RUST

jj describe -m "feat(server): add TCP listener with basic accept loop" 2>/dev/null

# Create the review while changes are in @
R3=$(crit_as "quiet-owl" --json reviews create \
	--title "Server: add TCP listener" \
	--desc "Replace placeholder with actual TcpListener. Basic accept loop with error logging." \
	2>/dev/null | extract_id review_id)

jj new 2>/dev/null

echo "Review 3: $R3" >&2

crit_as "swift-falcon" comment "$R3" --file src/server.rs --line 5 \
	"We decided to use tokio for async I/O instead. Let's close this and start fresh with an async approach." >/dev/null 2>&1

crit_as "quiet-owl" reviews abandon "$R3" \
	--reason "Superseded by async server approach" >/dev/null 2>&1

echo "  1 thread, abandoned" >&2

# ============================================================================
# Summary
# ============================================================================

echo "" >&2
echo "=== Demo project created at: $DEMO_DIR ===" >&2
echo "" >&2
echo "Reviews:" >&2
echo "  $R1  (open, active discussion)" >&2
echo "  $R2  (merged)" >&2
echo "  $R3  (abandoned)" >&2
echo "" >&2
echo "Try:" >&2
echo "  cd $DEMO_DIR" >&2
echo "  crit --agent demo-viewer reviews list" >&2
echo "  crit --agent demo-viewer review $R1" >&2
echo "  crit --agent bold-tiger inbox" >&2
echo "" >&2

echo "$DEMO_DIR"
