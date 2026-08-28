.DEFAULT_GOAL := build
.PHONY: build test lint fmt fmt-check install run clean

build: fmt-check lint test
	cargo build --release

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
