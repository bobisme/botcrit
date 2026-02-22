#!/usr/bin/env bash
# Capture screenshots of crit demo output for documentation.
# Requires: kitty, hyprctl (Hyprland), grim, jq, pngquant, tmux
#
# Usage:
#   ./scripts/screenshot-demo.sh              # Generate demo + capture all screenshots
#   ./scripts/screenshot-demo.sh /path/demo   # Use existing demo dir
#
# Output: images/*.png

set -euo pipefail

cd "$(dirname "$0")/.."
PROJECT_ROOT="$(pwd)"

CRIT="$PROJECT_ROOT/target/release/crit"
OUTPUT_DIR="$PROJECT_ROOT/images"
TMUX_SESSION="crit-screenshot"
WINDOW_TITLE="crit-screenshot"

TUI_WIDTH="${SCREENSHOT_TUI_WIDTH:-1200}"
TUI_HEIGHT="${SCREENSHOT_TUI_HEIGHT:-800}"
CLI_WIDTH="${SCREENSHOT_WIDTH:-1000}"
CLI_HEIGHT="${SCREENSHOT_HEIGHT:-700}"

# Build if needed
if [[ ! -x "$CRIT" ]]; then
	echo "Building crit release binary..." >&2
	cargo build --release --quiet
fi

# Generate or reuse demo
if [[ -n "${1:-}" ]]; then
	DEMO_DIR="$1"
else
	echo "Generating demo project..." >&2
	DEMO_DIR=$(./scripts/generate-demo-jj.sh 2>/dev/null)
fi
echo "Using demo at: $DEMO_DIR" >&2

# Get the open review ID
OPEN_REVIEW=$(cd "$DEMO_DIR" && "$CRIT" --agent viewer --json reviews list 2>/dev/null |
	jq -r '.[] | select(.status == "open") | .review_id')
echo "Open review: $OPEN_REVIEW" >&2

mkdir -p "$OUTPUT_DIR"

# Clean up any previous tmux session
tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true

# ============================================================================
# Helper: launch kitty with tmux, send keys, screenshot
# ============================================================================

# Launch a kitty window running a tmux session, wait for it to appear
launch_kitty_tmux() {
	local width="$1"
	local height="$2"

	# Create tmux session in detached mode with the demo dir (force bash for portability)
	tmux new-session -d -s "$TMUX_SESSION" -c "$DEMO_DIR" -x 120 -y 40 bash

	# Launch kitty attached to the tmux session
	kitty --title "$WINDOW_TITLE" \
		-o font_size=13 \
		-o window_padding_width=12 \
		-o background=#1e1e2e \
		-o foreground=#cdd6f4 \
		-e tmux attach-session -t "$TMUX_SESSION" &
	KITTY_PID=$!

	sleep 0.6

	# Position the window
	hyprctl dispatch focuswindow "title:$WINDOW_TITLE" >/dev/null 2>&1
	hyprctl dispatch togglefloating >/dev/null 2>&1
	hyprctl dispatch resizeactive exact "$width" "$height" >/dev/null 2>&1
	hyprctl dispatch centerwindow >/dev/null 2>&1

	sleep 0.4
}

# Capture the kitty window to a file
capture_kitty() {
	local output="$1"
	local tmp="/tmp/crit-screenshot-tmp.png"

	local geometry
	geometry=$(hyprctl clients -j | jq -r ".[] | select(.title == \"$WINDOW_TITLE\") | \"\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])\"")

	if [[ -z "$geometry" ]]; then
		echo "  Warning: Could not find window, skipping" >&2
		return 1
	fi

	grim -g "$geometry" "$tmp"
	pngquant --force --output "$output" "$tmp" 2>/dev/null
	rm -f "$tmp"

	local size
	size=$(du -h "$output" | cut -f1)
	echo "  Saved: $output ($size)" >&2
}

# Kill kitty and tmux
cleanup() {
	kill "$KITTY_PID" 2>/dev/null || true
	wait "$KITTY_PID" 2>/dev/null || true
	tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
}

# ============================================================================
# Capture TUI views
# ============================================================================

echo "Capturing TUI screenshots..." >&2

launch_kitty_tmux "$TUI_WIDTH" "$TUI_HEIGHT"

# Run crit ui
tmux send-keys -t "$TMUX_SESSION" "$CRIT --agent viewer ui" Enter
sleep 1.5

# Screenshot 1: Review list view
echo "  Capturing: tui-list" >&2
capture_kitty "$OUTPUT_DIR/tui-list.png"

# Navigate to top of list and enter the first review (open auth review)
tmux send-keys -t "$TMUX_SESSION" g
sleep 0.2
tmux send-keys -t "$TMUX_SESSION" Enter
sleep 1.0

# Scroll to bottom to show comments/threads
tmux send-keys -t "$TMUX_SESSION" G
sleep 0.5

# Screenshot 2: Review detail view (with diff + comments)
echo "  Capturing: tui-review" >&2
capture_kitty "$OUTPUT_DIR/tui-review.png"

# Quit the TUI
tmux send-keys -t "$TMUX_SESSION" q
sleep 0.3

cleanup

# ============================================================================
# Capture CLI views (optional)
# ============================================================================

if [[ "${ALL_SCREENSHOTS:-}" == "1" ]] || [[ "${2:-}" == "--all" ]]; then
	capture_cli() {
		local name="$1"
		shift
		local cmd="$*"

		echo "Capturing: $name" >&2

		tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
		tmux new-session -d -s "$TMUX_SESSION" -c "$DEMO_DIR" -x 120 -y 40 bash

		kitty --title "$WINDOW_TITLE" \
			-o font_size=13 \
			-o window_padding_width=16 \
			-o background=#1e1e2e \
			-o foreground=#cdd6f4 \
			-e tmux attach-session -t "$TMUX_SESSION" &
		KITTY_PID=$!

		sleep 0.6
		hyprctl dispatch focuswindow "title:$WINDOW_TITLE" >/dev/null 2>&1
		hyprctl dispatch togglefloating >/dev/null 2>&1
		hyprctl dispatch resizeactive exact "$CLI_WIDTH" "$CLI_HEIGHT" >/dev/null 2>&1
		hyprctl dispatch centerwindow >/dev/null 2>&1
		sleep 0.4

		tmux send-keys -t "$TMUX_SESSION" "$cmd" Enter
		sleep 0.5

		capture_kitty "$OUTPUT_DIR/$name.png"
		cleanup
	}

	capture_cli "reviews-list" "$CRIT --agent viewer reviews list"
	capture_cli "review" "$CRIT --agent viewer review $OPEN_REVIEW"
	capture_cli "threads-list" "$CRIT --agent viewer threads list $OPEN_REVIEW -v"
	capture_cli "inbox" "$CRIT --agent swift-falcon inbox"
	capture_cli "doctor" "$CRIT --agent viewer doctor"
fi

echo "" >&2
echo "All screenshots saved to $OUTPUT_DIR/" >&2
ls -lh "$OUTPUT_DIR"/*.png 2>/dev/null >&2
