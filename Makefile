.DEFAULT_GOAL := build
.PHONY: build check test lint fmt fmt-check install run clean

build: check
	cargo build --release

check: fmt-check lint test

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

install:
	cargo install --path .

run:
	cargo run -- $(ARGS)

clean:
	cargo clean
