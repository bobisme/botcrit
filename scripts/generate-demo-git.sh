#!/usr/bin/env bash
# Generate a Git-based demo project with realistic crit data.
#
# Usage:
#   ./scripts/generate-demo-git.sh          # Creates demo in /tmp/crit-demo-git-XXXXXX
#   ./scripts/generate-demo-git.sh /path    # Creates demo at custom path

set -euo pipefail

cd "$(dirname "$0")/.."

DEMO_DIR="${1:-$(mktemp -d /tmp/crit-demo-git-XXXXXX)}"
CRIT="$(pwd)/target/release/crit"

# Build a fresh release binary (demo scripts rely on current CLI flags)
echo "Building crit release binary..." >&2
cargo build --release --quiet

# Clean slate
if [[ -d "$DEMO_DIR" ]] && [[ -d "$DEMO_DIR/.git" ]]; then
	echo "Removing existing demo at $DEMO_DIR..." >&2
	rm -rf "$DEMO_DIR"
	mkdir -p "$DEMO_DIR"
fi

cd "$DEMO_DIR"

# ==========================================================================
# Set up a Git repository with sample source files
# ==========================================================================

git init -q
git config user.name "Demo User"
git config user.email "demo@example.com"

mkdir -p src

cat >src/main.rs <<'RUST'
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

cat >src/auth.rs <<'RUST'
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

cat >src/config.rs <<'RUST'
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

cat >src/server.rs <<'RUST'
use crate::config::Config;

pub fn run(addr: &str, config: &Config) {
    println!("Connecting to database: {}", config.database_url);
    println!("Max connections: {}", config.max_connections);
    println!("Listening on {}", addr);
}
RUST

cat >Cargo.toml <<'TOML'
[package]
name = "demo-app"
version = "0.1.0"
edition = "2021"
TOML

cat >README.md <<'MD'
# demo-app

A sample web server for demonstrating crit code review.
MD

git add .
git commit -q -m "feat: initial project structure"

"$CRIT" --agent setup-bot init >/dev/null 2>&1

crit_as() {
	local agent="$1"
	shift
	"$CRIT" --agent "$agent" --scm git "$@"
}

extract_id() {
	local field="$1"
	jq -r ".$field"
}

# ==========================================================================
# Review 1: Auth refactor (open, active discussion)
# ==========================================================================

git switch -c demo/auth-review >/dev/null 2>&1

cat >src/main.rs <<'RUST'
use std::env;

mod auth;
mod config;
mod server;

fn main() {
    let config = config::load();
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());

    println!("Starting server on {}", addr);
    auth::init_sessions();
    server::run(&addr, &config);
}
RUST

cat >src/auth.rs <<'RUST'
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

git add src/main.rs src/auth.rs
git commit -q -m "refactor(auth): replace unsafe static with RwLock"

R1=$(crit_as "swift-falcon" --json reviews create \
	--title "Refactor auth: replace unsafe static with RwLock" \
	--description "Replaces unsafe mutable static session state with an RwLock and adds session lifecycle helpers." \
	2>/dev/null | extract_id review_id)

crit_as "swift-falcon" reviews request "$R1" --reviewers "bold-tiger,quiet-owl" >/dev/null 2>&1

crit_as "bold-tiger" comment "$R1" --file src/auth.rs --line 4 \
	"Nice — removing the unsafe block is a big improvement." >/dev/null 2>&1

T_SIZE=$(crit_as "bold-tiger" --json comment "$R1" --file src/auth.rs --line 14 \
	"Should we bound the session map size? In production this could grow unbounded if sessions aren't cleaned up." \
	2>/dev/null | extract_id thread_id)

crit_as "quiet-owl" comment "$R1" --file src/auth.rs --line 43 \
	"fastrand isn't cryptographically secure. For session tokens, use rand::OsRng or similar." >/dev/null 2>&1

crit_as "swift-falcon" reply "$T_SIZE" \
	"Good point. I'll add max session limits in a follow-up." >/dev/null 2>&1

crit_as "bold-tiger" lgtm "$R1" -m "Looks good overall. The unsafe removal is solid." >/dev/null 2>&1
crit_as "quiet-owl" block "$R1" -r "Need cryptographically secure token generation before merge" >/dev/null 2>&1

echo "Review 1: $R1 (open, 1 LGTM + 1 block)" >&2

git switch main >/dev/null 2>&1

# ==========================================================================
# Review 2: Config improvements (merged)
# ==========================================================================

git switch -c demo/config-review >/dev/null 2>&1

cat >src/config.rs <<'RUST'
use std::env;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub max_connections: u32,
    pub log_level: String,
    pub bind_addr: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database_url: "postgres://localhost/app".into(),
            max_connections: 10,
            log_level: "info".into(),
            bind_addr: "0.0.0.0:8080".into(),
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

    config
}
RUST

git add src/config.rs
git commit -q -m "feat(config): add defaults and env overrides"

R2_JSON=$(crit_as "bold-tiger" --json reviews create \
	--title "Config: add Default impl and env var overrides" \
	--description "All config fields now have defaults and can be overridden via env vars." \
	2>/dev/null)
R2=$(jq -r '.review_id' <<<"$R2_JSON")
R2_COMMIT=$(jq -r '.initial_commit' <<<"$R2_JSON")

crit_as "bold-tiger" reviews request "$R2" --reviewers "swift-falcon" >/dev/null 2>&1
T_BUILDER=$(crit_as "swift-falcon" --json comment "$R2" --file src/config.rs --line 22 \
	"Consider using a builder pattern instead of mutating fields." \
	2>/dev/null | extract_id thread_id)
crit_as "bold-tiger" reply "$T_BUILDER" "Reasonable, but this is concise for now." >/dev/null 2>&1
crit_as "swift-falcon" threads resolve "$T_BUILDER" --reason "Agreed to defer" >/dev/null 2>&1

crit_as "swift-falcon" lgtm "$R2" -m "Clean implementation. Good defaults." >/dev/null 2>&1
crit_as "bold-tiger" reviews mark-merged "$R2" --commit "$R2_COMMIT" >/dev/null 2>&1

echo "Review 2: $R2 (merged)" >&2

git switch main >/dev/null 2>&1

# ==========================================================================
# Review 3: Server work (abandoned)
# ==========================================================================

git switch -c demo/server-review >/dev/null 2>&1

cat >src/server.rs <<'RUST'
use crate::config::Config;
use std::io;
use std::net::TcpListener;

pub fn run(addr: &str, config: &Config) -> io::Result<()> {
    println!("Connecting to {}", config.database_url);
    let listener = TcpListener::bind(addr)?;
    println!("Listening on {}", addr);

    for stream in listener.incoming() {
        match stream {
            Ok(_stream) => {}
            Err(e) => eprintln!("Connection failed: {}", e),
        }
    }
    Ok(())
}
RUST

git add src/server.rs
git commit -q -m "feat(server): add tcp listener loop"

R3=$(crit_as "quiet-owl" --json reviews create \
	--title "Server: add TCP listener" \
	--description "Replace placeholder server loop with TcpListener accept loop." \
	2>/dev/null | extract_id review_id)

crit_as "swift-falcon" comment "$R3" --file src/server.rs --line 5 \
	"Let's supersede this with the async server approach." >/dev/null 2>&1
crit_as "quiet-owl" reviews abandon "$R3" --reason "Superseded by async server design" >/dev/null 2>&1

echo "Review 3: $R3 (abandoned)" >&2

git switch main >/dev/null 2>&1

# ==========================================================================
# Review 4: Bare open review for manual testing
# ==========================================================================

git switch -c demo/logging-review >/dev/null 2>&1

cat >src/logging.rs <<'RUST'
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

pub fn log(level: LogLevel, message: &str) {
    let level = match level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    };
    println!("[{}] {}", level, message);
}
RUST

git add src/logging.rs
git commit -q -m "feat(logging): add basic logging module"

R4=$(crit_as "mystic-pine" --json reviews create \
	--title "Logging: add basic logging module" \
	--description "New logging.rs with levels and a basic log() helper." \
	2>/dev/null | extract_id review_id)

echo "Review 4: $R4 (open, no threads)" >&2

git switch main >/dev/null 2>&1

echo "" >&2
echo "=== Git demo project created at: $DEMO_DIR ===" >&2
echo "  $R1  (open)" >&2
echo "  $R2  (merged)" >&2
echo "  $R3  (abandoned)" >&2
echo "  $R4  (open, bare)" >&2
echo "" >&2
echo "Try:" >&2
echo "  cd $DEMO_DIR" >&2
echo "  crit --agent demo-viewer --scm git reviews list" >&2
echo "  crit --agent demo-viewer --scm git review $R1" >&2
echo "  crit --agent bold-tiger --scm git inbox" >&2
echo "" >&2

echo "$DEMO_DIR"
