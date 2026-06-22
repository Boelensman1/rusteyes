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
CARGO_SOURCES := Cargo.toml Cargo.lock $(shell find src -type f -name '*.rs' 2>/dev/null)
MACOS_HELPER_DIR := helpers/macos-helper
MACOS_HELPER_BIN := $(MACOS_HELPER_DIR)/.build/debug/rusteyes-macos-helper
MACOS_HELPER_SOURCES := $(MACOS_HELPER_DIR)/Package.swift $(shell find $(MACOS_HELPER_DIR)/Sources -type f -name '*.swift' 2>/dev/null)
MACOS_APP_DIR := target/macos/RustEyes.app
MACOS_APP_CONTENTS := $(MACOS_APP_DIR)/Contents
MACOS_APP_BIN := $(MACOS_APP_CONTENTS)/MacOS/rusteyes
MACOS_APP_HELPER := $(MACOS_APP_CONTENTS)/Resources/rusteyes-macos-helper
MACOS_APP_ICON := package/macos/RustEyes.icns
MACOS_APP_PLIST := package/macos/Info.plist
LSREGISTER := /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
ifeq ($(UNAME_S),Darwin)
RUN_DEPS := $(MACOS_APP_BIN)
endif

.PHONY: help run fmt fmt-check lint test check build macos-helper-build macos-app-build clean

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
		'  make macos-app-build     Build the macOS app bundle on Darwin' \
		'  make clean      Remove Cargo build output'

run: $(RUN_DEPS)
ifeq ($(UNAME_S),Darwin)
	$(MACOS_APP_BIN)
else
	$(CARGO_RUN) run
endif

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

macos-app-build: $(MACOS_APP_BIN)

target/debug/rusteyes: $(CARGO_SOURCES)
	$(CARGO_RUN) build

$(MACOS_APP_BIN): target/debug/rusteyes $(MACOS_HELPER_BIN) $(MACOS_APP_ICON) $(MACOS_APP_PLIST) Makefile
ifeq ($(UNAME_S),Darwin)
	rm -rf $(MACOS_APP_DIR)
	mkdir -p $(MACOS_APP_CONTENTS)/MacOS $(MACOS_APP_CONTENTS)/Resources
	cp $(MACOS_APP_PLIST) $(MACOS_APP_CONTENTS)/Info.plist
	cp target/debug/rusteyes $(MACOS_APP_BIN)
	cp $(MACOS_HELPER_BIN) $(MACOS_APP_HELPER)
	cp $(MACOS_APP_ICON) $(MACOS_APP_CONTENTS)/Resources/RustEyes.icns
	chmod +x $(MACOS_APP_BIN) $(MACOS_APP_HELPER)
	codesign --force --deep --sign - $(MACOS_APP_DIR)
	$(LSREGISTER) -f $(MACOS_APP_DIR)
else
	@printf '%s\n' 'Skipping macOS app bundle build on $(UNAME_S)'
endif

clean:
	$(CARGO_RUN) clean
