build:
  cargo build --release

test:
  cargo test

install:
  cargo install --locked --force --path crates/crit-cli
