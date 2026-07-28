PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR

DEBUG_DIR := $(CARGO_TARGET_DIR)/debug
RELEASE_DIR := $(CARGO_TARGET_DIR)/release
DEBUG_GUI := $(DEBUG_DIR)/hwall
DEBUG_CLI := $(DEBUG_DIR)/hwall-cli
RELEASE_GUI := $(RELEASE_DIR)/hwall
RELEASE_CLI := $(RELEASE_DIR)/hwall-cli

MANIFESTS := Cargo.toml $(wildcard crates/*/Cargo.toml)
CORE_INPUTS := $(shell find crates/hwall-core -type f -print)
APP_INPUTS := $(shell find crates/hwall-app -type f -print)
GUI_INPUTS := $(CORE_INPUTS) $(APP_INPUTS) $(shell find crates/hwall-gui -type f -print) $(MANIFESTS)
CLI_INPUTS := $(CORE_INPUTS) $(APP_INPUTS) $(shell find crates/hwall-cli -type f -print) $(MANIFESTS)

.PHONY: lock build release release-gui release-cli check test lint format \
	verify-format verify-source install install-gui install-cli clean

Cargo.lock: $(MANIFESTS)
	cargo generate-lockfile

lock: Cargo.lock
	@cargo metadata --locked --format-version 1 >/dev/null

$(DEBUG_GUI): Cargo.lock $(GUI_INPUTS)
	cargo build --locked -p hwall-gui

$(DEBUG_CLI): Cargo.lock $(CLI_INPUTS)
	cargo build --locked -p hwall-cli

build: lock $(DEBUG_GUI) $(DEBUG_CLI)

$(RELEASE_GUI): Cargo.lock $(GUI_INPUTS)
	cargo build --locked -p hwall-gui --release

$(RELEASE_CLI): Cargo.lock $(CLI_INPUTS)
	cargo build --locked -p hwall-cli --release

release-gui: lock $(RELEASE_GUI)

release-cli: lock $(RELEASE_CLI)

release: release-gui release-cli

check: lock verify-format verify-source
	cargo check --locked --workspace --all-targets

verify-format:
	cargo fmt --all -- --check

verify-source:
	python3 scripts/check-source.py

test: lock
	cargo test --locked --workspace --all-features

lint: lock
	cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

format:
	cargo fmt --all

install: install-gui install-cli

install-gui: release-gui
	install -Dm755 "$(RELEASE_GUI)" "$(DESTDIR)$(BINDIR)/hwall"
	install -Dm644 packaging/io.github.hwall.HWall.desktop \
		"$(DESTDIR)$(DATADIR)/applications/io.github.hwall.HWall.desktop"
	install -Dm644 packaging/io.github.hwall.HWall.metainfo.xml \
		"$(DESTDIR)$(DATADIR)/metainfo/io.github.hwall.HWall.metainfo.xml"
	install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.hwall.HWall.svg \
		"$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/io.github.hwall.HWall.svg"
	for size in 32 48 64; do \
		install -Dm644 "packaging/icons/hicolor/$${size}x$${size}/apps/io.github.hwall.HWall.png" \
			"$(DESTDIR)$(DATADIR)/icons/hicolor/$${size}x$${size}/apps/io.github.hwall.HWall.png"; \
	done

install-cli: release-cli
	install -Dm755 "$(RELEASE_CLI)" "$(DESTDIR)$(BINDIR)/hwall-cli"

clean:
	cargo clean
