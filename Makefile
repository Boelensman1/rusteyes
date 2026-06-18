.DEFAULT_GOAL := help

CARGO := $(shell command -v cargo 2>/dev/null)
ifeq ($(strip $(CARGO)),)
CARGO_RUN := nix develop --command cargo
else
CARGO_RUN := cargo
endif

.PHONY: help run fmt fmt-check lint test check build clean

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

clean:
	$(CARGO_RUN) clean
