# FS9 Distributed File System
# Makefile for common development tasks

.PHONY: all build test clean fmt lint check doc server install help
.PHONY: test-rust test-python test-e2e test-unit
.PHONY: dev setup-python clean-all
.PHONY: plugins release-all admin-cli

# Default target - build everything
all: build plugins

# =============================================================================
# Build
# =============================================================================

## Build all Rust crates (debug)
build:
	cargo build --workspace

## Build plugins and copy to ./plugins directory for auto-loading
plugins:
	cargo build --release -p fs9-plugin-pagefs -p fs9-plugin-streamfs -p fs9-plugin-kv -p fs9-plugin-hellofs -p fs9-plugin-pubsubfs
	@mkdir -p plugins
	@cp -f target/release/libfs9_plugin_pagefs.so plugins/ 2>/dev/null || \
	 cp -f target/release/libfs9_plugin_pagefs.dylib plugins/ 2>/dev/null || true
	@cp -f target/release/libfs9_plugin_streamfs.so plugins/ 2>/dev/null || \
	 cp -f target/release/libfs9_plugin_streamfs.dylib plugins/ 2>/dev/null || true
	@cp -f target/release/libfs9_plugin_kv.so plugins/ 2>/dev/null || \
	 cp -f target/release/libfs9_plugin_kv.dylib plugins/ 2>/dev/null || true
	@cp -f target/release/libfs9_plugin_hellofs.so plugins/ 2>/dev/null || \
	 cp -f target/release/libfs9_plugin_hellofs.dylib plugins/ 2>/dev/null || true
	@cp -f target/release/libfs9_plugin_pubsubfs.so plugins/ 2>/dev/null || \
	 cp -f target/release/libfs9_plugin_pubsubfs.dylib plugins/ 2>/dev/null || true
	@echo "Plugins installed to ./plugins/"
	@ls -la plugins/*.so plugins/*.dylib 2>/dev/null || true

## Build everything in release mode
release: release-all

release-all:
	cargo build --workspace --release

## Build only the server
server-build:
	cargo build -p fs9-server

## Build the admin CLI (multi-tenant management)
admin-cli:
	cargo build --release -p fs9-cli
	@echo ""
	@echo "Admin CLI built: target/release/fs9-admin"
	@echo "Usage: ./target/release/fs9-admin --help"

# =============================================================================
# Test
# =============================================================================

## Run all tests (Rust + Python)
test: test-rust test-python

## Run all Rust tests
test-rust:
	cargo test --workspace

## Run unit tests only (no E2E)
test-unit:
	cargo test --workspace --exclude fs9-tests

## Run E2E integration tests
test-e2e:
	cargo build -p fs9-server
	cargo test -p fs9-tests

## Run Python tests
test-python:
	cd clients/python && \
		source .venv/bin/activate && \
		pytest -v

## Run Python E2E tests only
test-python-e2e:
	cd clients/python && \
		source .venv/bin/activate && \
		pytest tests/test_e2e.py -v

# =============================================================================
# Code Quality
# =============================================================================

## Format all code
fmt:
	cargo fmt --all
	cd clients/python && \
		source .venv/bin/activate && \
		ruff format . || true

## Run linter
lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cd clients/python && \
		source .venv/bin/activate && \
		ruff check . || true

## Run all checks (fmt + lint + test)
check: fmt lint test

## Check formatting without modifying
fmt-check:
	cargo fmt --all -- --check

# =============================================================================
# Documentation
# =============================================================================

## Generate documentation
doc:
	cargo doc --workspace --no-deps

## Open documentation in browser
doc-open:
	cargo doc --workspace --no-deps --open

# =============================================================================
# Development
# =============================================================================

## Run the server (development mode)
server:
	RUST_LOG=info cargo run -p fs9-server

## Run the server on custom port
## Usage: make server-port PORT=8080
server-port:
	RUST_LOG=info FS9_PORT=$(PORT) cargo run -p fs9-server

## Watch and rebuild on changes (requires cargo-watch)
dev:
	cargo watch -x 'build --workspace'

## Setup Python development environment
setup-python:
	cd clients/python && \
		python3 -m venv .venv && \
		source .venv/bin/activate && \
		pip install -e ".[dev]"

# =============================================================================
# Installation
# =============================================================================

## Install server binary
install:
	cargo install --path server

## Install to custom location
## Usage: make install-to PREFIX=/usr/local
install-to:
	cargo install --path server --root $(PREFIX)

# =============================================================================
# Clean
# =============================================================================

## Clean Rust build artifacts
clean:
	cargo clean

## Clean everything including Python venv
clean-all: clean
	rm -rf clients/python/.venv
	rm -rf clients/python/.pytest_cache
	rm -rf clients/python/**/__pycache__
	find . -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
	find . -type f -name "*.pyc" -delete 2>/dev/null || true

# =============================================================================
# Help
# =============================================================================

## Show this help
help:
	@echo "FS9 - Plan 9 Inspired Distributed File System"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Build:"
	@echo "  all            Build everything (debug + plugins copied to ./plugins)"
	@echo "  build          Build all Rust crates (debug)"
	@echo "  plugins        Build plugins (release) and copy to ./plugins"
	@echo "  release        Build everything in release mode"
	@echo "  server-build   Build only the server"
	@echo "  admin-cli      Build the admin CLI (multi-tenant management)"
	@echo ""
	@echo "Test:"
	@echo "  test           Run all tests (Rust + Python)"
	@echo "  test-rust      Run all Rust tests"
	@echo "  test-unit      Run unit tests only (no E2E)"
	@echo "  test-e2e       Run E2E integration tests"
	@echo "  test-python    Run Python tests"
	@echo ""
	@echo "Code Quality:"
	@echo "  fmt            Format all code"
	@echo "  lint           Run linter"
	@echo "  check          Run all checks (fmt + lint + test)"
	@echo ""
	@echo "Documentation:"
	@echo "  doc            Generate documentation"
	@echo "  doc-open       Open documentation in browser"
	@echo ""
	@echo "Development:"
	@echo "  server         Run the server (development mode)"
	@echo "  server-port    Run server on custom port (PORT=8080)"
	@echo "  dev            Watch and rebuild on changes"
	@echo "  setup-python   Setup Python development environment"
	@echo ""
	@echo "Installation:"
	@echo "  install        Install server binary"
	@echo ""
	@echo "Clean:"
	@echo "  clean          Clean Rust build artifacts"
	@echo "  clean-all      Clean everything including Python venv"
