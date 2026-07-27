PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
CARGO_TARGET_DIR ?= target
RELEASE_DIR := $(CARGO_TARGET_DIR)/release

.PHONY: lock build release release-cli check test lint format verify-format verify-source install install-cli

lock:
	@cargo metadata --locked --no-deps --format-version 1 >/dev/null 2>&1 || cargo generate-lockfile

build: lock
	cargo build --locked --workspace

release: lock
	cargo build --locked --workspace --release

release-cli: lock
	cargo build --locked -p hwall-cli --release

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

install: release
	install -Dm755 "$(RELEASE_DIR)/hwall" "$(DESTDIR)$(BINDIR)/hwall"
	install -Dm755 "$(RELEASE_DIR)/hwall-cli" "$(DESTDIR)$(BINDIR)/hwall-cli"
	install -Dm644 packaging/io.github.hwall.HWall.desktop \
		"$(DESTDIR)$(DATADIR)/applications/io.github.hwall.HWall.desktop"
	install -Dm644 packaging/io.github.hwall.HWall.metainfo.xml \
		"$(DESTDIR)$(DATADIR)/metainfo/io.github.hwall.HWall.metainfo.xml"

install-cli: release-cli
	install -Dm755 "$(RELEASE_DIR)/hwall-cli" "$(DESTDIR)$(BINDIR)/hwall-cli"
