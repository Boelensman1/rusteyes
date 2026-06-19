.DEFAULT_GOAL := help

CARGO := $(shell command -v cargo 2>/dev/null)
SWIFT := $(shell command -v swift 2>/dev/null)
UNAME_S := $(shell uname -s)
ifeq ($(strip $(CARGO)),)
CARGO_RUN := nix develop --command cargo
else
CARGO_RUN := cargo
endif
SWIFT_BUILD_ENV := env -u DEVELOPER_DIR -u SDKROOT -u MACOSX_DEPLOYMENT_TARGET
MACOS_HELPER_DIR := helpers/macos-helper
MACOS_HELPER_BIN := $(MACOS_HELPER_DIR)/.build/debug/resteyes-macos-helper
MACOS_HELPER_SOURCES := $(MACOS_HELPER_DIR)/Package.swift $(shell find $(MACOS_HELPER_DIR)/Sources -type f -name '*.swift' 2>/dev/null)

.PHONY: help run fmt fmt-check lint test check build macos-helper-build clean

help:
	@printf '%s\n' \
		'Available targets:' \
		'  make run        Run the application' \
		'  make fmt        Format Rust code' \
		'  make fmt-check  Check Rust formatting' \
		'  make lint       Run Clippy with warnings denied' \
		'  make test       Run tests' \
		'  make check      Run formatting, linting, and tests' \
		'  make build      Build the application' \
		'  make macos-helper-build  Build the macOS helper on Darwin' \
		'  make clean      Remove Cargo build output'

run:
	$(CARGO_RUN) run

fmt:
	$(CARGO_RUN) fmt --all

fmt-check:
	$(CARGO_RUN) fmt --all --check

lint:
	$(CARGO_RUN) clippy --all-targets --all-features -- -D warnings

test:
	$(CARGO_RUN) test --all-targets --all-features

check: fmt-check lint test

build:
	$(CARGO_RUN) build

macos-helper-build: $(MACOS_HELPER_BIN)

$(MACOS_HELPER_BIN): $(MACOS_HELPER_SOURCES)
ifeq ($(UNAME_S),Darwin)
ifeq ($(strip $(SWIFT)),)
	$(error swift is required to build the macOS helper)
endif
	$(SWIFT_BUILD_ENV) $(SWIFT) build --package-path $(MACOS_HELPER_DIR)
else
	@printf '%s\n' 'Skipping macOS helper build on $(UNAME_S)'
endif

clean:
	$(CARGO_RUN) clean
