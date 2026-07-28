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

WORKSPACE_INPUTS := Cargo.lock Cargo.toml rust-toolchain.toml \
	$(shell find crates -type f -print)

.PHONY: build release release-gui release-cli check test lint format \
	verify-format verify-source install install-gui install-cli clean

# One Cargo invocation produces both workspace binaries. Grouped targets also let
# make reuse current binaries without invoking Cargo again.
$(DEBUG_GUI) $(DEBUG_CLI) &: $(WORKSPACE_INPUTS)
	cargo build --locked --workspace

build: $(DEBUG_GUI) $(DEBUG_CLI)

$(RELEASE_GUI) $(RELEASE_CLI) &: $(WORKSPACE_INPUTS)
	cargo build --locked --workspace --release

release: $(RELEASE_GUI) $(RELEASE_CLI)

release-gui: $(RELEASE_GUI)

release-cli: $(RELEASE_CLI)

check: Cargo.lock verify-format verify-source
	cargo check --locked --workspace --all-targets

verify-format:
	cargo fmt --all -- --check

verify-source:
	python3 scripts/check-source.py

test: Cargo.lock
	cargo test --locked --workspace --all-features

lint: Cargo.lock
	cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

format:
	cargo fmt --all

install: release
	install -Dm755 "$(RELEASE_GUI)" "$(DESTDIR)$(BINDIR)/hwall"
	install -Dm755 "$(RELEASE_CLI)" "$(DESTDIR)$(BINDIR)/hwall-cli"
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
