#!/usr/bin/env python3
"""Fast source-tree, architecture, metadata, and package-hygiene checks."""

from __future__ import annotations

from pathlib import Path
import hashlib
import re
import sys
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
CORE = CRATES / "hwall-core"
APP = CRATES / "hwall-app"
CLI = CRATES / "hwall-cli"
GUI = CRATES / "hwall-gui"

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def manifest(path: Path) -> dict:
    return tomllib.loads(read(path))


def rust_sources(path: Path) -> list[Path]:
    return sorted(path.rglob("*.rs"))


def production_source(path: Path) -> str:
    return read(path).split("#[cfg(test)]", 1)[0]


def release_files() -> list[Path]:
    return sorted(
        path
        for path in ROOT.rglob("*")
        if path.is_file()
        and path.name != "SHA256SUMS"
        and not any(part in {".git", "target"} for part in path.relative_to(ROOT).parts)
    )


def check_checksums() -> None:
    checksum_path = ROOT / "SHA256SUMS"
    entries: dict[str, str] = {}
    for line_number, line in enumerate(read(checksum_path).splitlines(), start=1):
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            fail(f"SHA256SUMS:{line_number}: invalid checksum entry")
            continue
        checksum, relative = match.groups()
        if relative in entries:
            fail(f"SHA256SUMS lists {relative!r} more than once")
        entries[relative] = checksum

    files = {path.relative_to(ROOT).as_posix(): path for path in release_files()}
    if set(entries) != set(files):
        missing = sorted(set(files) - set(entries))
        extra = sorted(set(entries) - set(files))
        fail(f"SHA256SUMS file list differs from the source tree; missing={missing}, extra={extra}")
        return

    for relative, path in files.items():
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if entries[relative] != actual:
            fail(f"SHA256SUMS mismatch for {relative}")


def check_workspace() -> None:
    workspace = manifest(ROOT / "Cargo.toml")
    expected_members = {
        "crates/hwall-core",
        "crates/hwall-app",
        "crates/hwall-cli",
        "crates/hwall-gui",
    }
    members = set(workspace.get("workspace", {}).get("members", []))
    if members != expected_members:
        fail(f"workspace members differ from the four expected crates: {sorted(members)}")

    internal_dependencies = {
        crate: {
            name
            for name in manifest(CRATES / crate / "Cargo.toml")
            .get("dependencies", {})
            .keys()
            if name.startswith("hwall-")
        }
        for crate in ["hwall-core", "hwall-app", "hwall-cli", "hwall-gui"]
    }
    expected_dependencies = {
        "hwall-core": set(),
        "hwall-app": {"hwall-core"},
        "hwall-cli": {"hwall-core"},
        "hwall-gui": {"hwall-core", "hwall-app"},
    }
    if internal_dependencies != expected_dependencies:
        fail(f"crate dependency direction changed: {internal_dependencies}")

    package_defaults = workspace.get("workspace", {}).get("package", {})
    for crate in expected_members:
        package = manifest(ROOT / crate / "Cargo.toml").get("package", {})
        for field in ["version", "edition", "rust-version"]:
            inherited = package.get(field)
            if not isinstance(inherited, dict) or inherited.get("workspace") is not True:
                fail(f"{crate}/Cargo.toml must inherit workspace {field}")

    gui_manifest = manifest(GUI / "Cargo.toml")
    gui_dependencies = gui_manifest.get("dependencies", {})
    gtk = gui_dependencies.get("gtk", {})
    if not isinstance(gtk, dict) or "v4_8" not in gtk.get("features", []):
        fail("the GUI must target the documented GTK 4.8 API baseline")
    if "directories" in gui_dependencies:
        fail("desktop path policy belongs in hwall-app, not hwall-gui")
    for crate in [CORE, APP, CLI]:
        dependencies = manifest(crate / "Cargo.toml").get("dependencies", {})
        if set(dependencies) & {"gtk", "gtk4"}:
            fail(f"{crate.name} must remain independent of GTK")

    tray = gui_manifest.get("features", {}).get("tray", [])
    ksni = gui_dependencies.get("ksni", {})
    if tray != ["dep:ksni"]:
        fail("the tray feature must enable only the optional ksni dependency")
    if not isinstance(ksni, dict) or not ksni.get("optional"):
        fail("ksni must remain an optional dependency")
    if ksni.get("default-features", True):
        fail("ksni default features must remain disabled")
    features = set(ksni.get("features", []))
    if "blocking" not in features or len(features & {"async-io", "tokio"}) != 1:
        fail("ksni must use the blocking API with exactly one zbus I/O backend")

    version = package_defaults.get("version")
    rust_version = package_defaults.get("rust-version")
    toolchain = manifest(ROOT / "rust-toolchain.toml").get("toolchain", {})
    if not version or not rust_version:
        fail("workspace package version and rust-version must be defined")
    if not toolchain.get("channel") or set(toolchain.get("components", [])) != {
        "clippy",
        "rustfmt",
    }:
        fail("rust-toolchain.toml must select a toolchain with clippy and rustfmt")


def check_metadata() -> None:
    workspace = manifest(ROOT / "Cargo.toml")
    version = workspace["workspace"]["package"]["version"]
    app_id = "io.github.hwall.HWall"

    desktop = read(ROOT / "packaging" / f"{app_id}.desktop")
    for line in ["Exec=hwall", "Terminal=false", f"StartupWMClass={app_id}"]:
        if line not in desktop.splitlines():
            fail(f"desktop entry is missing {line!r}")

    metadata_path = ROOT / "packaging" / f"{app_id}.metainfo.xml"
    try:
        metadata = ET.parse(metadata_path).getroot()
    except ET.ParseError as error:
        fail(f"invalid AppStream XML: {error}")
        return
    if metadata.findtext("id") != app_id:
        fail("AppStream component ID does not match the desktop application ID")
    if metadata.find("project_license") is not None:
        fail("AppStream metadata must not declare a project license until one is chosen")
    if metadata.find("releases") is not None:
        fail("AppStream metadata must not expose local pre-release history")
    if metadata.findtext("metadata_license") != "CC0-1.0":
        fail("AppStream metadata must retain its metadata-only CC0-1.0 license")
    if metadata.findtext("./launchable") != f"{app_id}.desktop":
        fail("AppStream launchable does not match the desktop file")

    rust_text = "\n".join(read(path) for path in rust_sources(CRATES))
    if rust_text.count(f'"{app_id}"') != 1:
        fail("the Rust source must define the application ID exactly once")
    if rust_text.count('"utilities-system-monitor"') != 1:
        fail("the Rust source must define the application icon exactly once")

    main = read(GUI / "src" / "main.rs")
    activate = re.search(r"fn activate\([^)]*\)\s*\{(.*?)\n\}", main, re.S)
    if (
        not activate
        or "gtk::Window::set_default_icon_name(APPLICATION_ICON);"
        not in activate.group(1)
    ):
        fail("the GTK default icon must be set inside activate(), after GTK initialization")

    plasma = read(APP / "src" / "plasma.rs")
    window = read(GUI / "src" / "window.rs")
    for token in ['("title", MAIN_WINDOW_TITLE)', '("titlematch", "1")']:
        if token not in plasma:
            fail(f"Plasma placement rule is missing {token!r}")
    if "MAIN_WINDOW_TITLE" not in window:
        fail("main-window construction must use the shared Plasma title")


def check_architecture_boundaries() -> None:
    gui_source = "\n".join(read(path) for path in rust_sources(GUI / "src"))
    tui_source = "\n".join(
        read(path)
        for path in [CLI / "src" / "tui.rs", *rust_sources(CLI / "src" / "tui")]
    )
    for label, source in [("GTK frontend", gui_source), ("terminal UI", tui_source)]:
        for token in ["collect_snapshot(", "std::process::Command", "Command::new("]:
            if token in source:
                fail(f"{label} crosses the monitor/collector boundary with {token!r}")
        if "MonitorWorker" not in source:
            fail(f"{label} must use the nonblocking monitor-worker boundary")

    for path in rust_sources(CRATES):
        source = production_source(path)
        for pattern, description in [
            (r"\.unwrap\s*\(", "unwrap"),
            (r"\.expect\s*\(", "expect"),
            (r"\bpanic!\s*\(", "panic"),
            (r"\bunreachable!\s*\(", "unreachable"),
            (r"\b(?:todo|unimplemented)!\s*\(", "unfinished macro"),
            (r"\bdbg!\s*\(", "debug macro"),
            (r"#\[allow\s*\(", "lint suppression"),
        ]:
            for match in re.finditer(pattern, source):
                line = source.count("\n", 0, match.start()) + 1
                fail(f"{path.relative_to(ROOT)}:{line}: production {description}")

    app_source = "\n".join(read(path) for path in rust_sources(APP / "src"))
    core_source = "\n".join(read(path) for path in rust_sources(CORE / "src"))
    cli_source = "\n".join(read(path) for path in rust_sources(CLI / "src"))
    for name, source, expected in [
        ("escape_delimited", f"{core_source}\n{app_source}\n{cli_source}", 1),
        ("hardware_property_label", f"{core_source}\n{app_source}", 1),
        ("is_virtual_block_device_name", core_source, 1),
    ]:
        count = len(re.findall(rf"\bfn\s+{name}\s*\(", source))
        if count != expected:
            fail(f"shared helper {name!r} has {count} definitions; expected {expected}")


def check_modules() -> None:
    declaration = re.compile(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.MULTILINE
    )
    for path in rust_sources(CRATES):
        base = (
            path.parent
            if path.name in {"lib.rs", "main.rs", "mod.rs"}
            else path.parent / path.stem
        )
        for name in declaration.findall(read(path)):
            flat = base / f"{name}.rs"
            nested = base / name / "mod.rs"
            if not flat.exists() and not nested.exists():
                fail(f"{path.relative_to(ROOT)} declares missing module {name!r}")


def check_css() -> None:
    css_path = GUI / "resources" / "style.css"
    css = read(css_path)
    blocks = re.findall(r"([^{}]+)\{([^{}]*)\}", css)
    selectors: set[str] = set()
    custom_classes: set[str] = set()
    for raw_selectors, body in blocks:
        for selector in raw_selectors.split(","):
            selector = " ".join(selector.split())
            if selector in selectors:
                fail(f"duplicate CSS selector: {selector}")
            selectors.add(selector)
            custom_classes.update(re.findall(r"\.([A-Za-z_][\w-]*)", selector))
        declarations: set[str] = set()
        for declaration in body.split(";"):
            if ":" not in declaration:
                continue
            property_name = declaration.split(":", 1)[0].strip()
            if property_name in declarations:
                fail(f"CSS block {raw_selectors.strip()!r} assigns {property_name!r} twice")
            declarations.add(property_name)

    gui_source = "\n".join(read(path) for path in rust_sources(GUI / "src"))
    used_classes = set(re.findall(r'"([A-Za-z_][\w-]*)"', gui_source))
    unused = sorted(custom_classes - used_classes)
    if unused:
        fail(f"custom CSS classes are not referenced by GTK source: {unused}")


def check_makefile() -> None:
    makefile = read(ROOT / "Makefile")
    required = [
        "release-cli: lock",
        "install: release",
        "install-cli: release-cli",
        "$(DESTDIR)$(BINDIR)/hwall",
        "$(DESTDIR)$(BINDIR)/hwall-cli",
        "$(DESTDIR)$(DATADIR)/applications/io.github.hwall.HWall.desktop",
        "$(DESTDIR)$(DATADIR)/metainfo/io.github.hwall.HWall.metainfo.xml",
    ]
    for token in required:
        if token not in makefile:
            fail(f"Makefile is missing {token!r}")
    for obsolete in ["install-gui", "install-all"]:
        if re.search(rf"(?m)^{re.escape(obsolete)}\s*:", makefile):
            fail(f"Makefile retains obsolete target {obsolete!r}")


def check_package_hygiene() -> None:
    ignored_directories = {".git", "target"}
    forbidden_directories = {"__pycache__"}
    forbidden_suffixes = {".pyc", ".pyo", ".swp", ".swo", ".bak", ".patch", ".log"}
    forbidden_name_prefixes = (
        "INTERNAL-CHANGELOG",
        "SOURCE-REVIEW",
        "SOURCE-VALIDATION",
    )
    forbidden_archive_suffixes = (".tar", ".tar.gz", ".tar.xz", ".tar.zst", ".tgz", ".zip")
    for path in ROOT.rglob("*"):
        relative = path.relative_to(ROOT)
        if any(part in ignored_directories for part in relative.parts):
            continue
        if any(part in forbidden_directories for part in relative.parts):
            fail(f"generated directory in source tree: {relative}")
            continue
        if not path.is_file():
            continue
        if path.name.startswith(forbidden_name_prefixes) or path.name.endswith("~"):
            fail(f"non-release file in source tree: {relative}")
        lower_name = path.name.lower()
        if path.suffix.lower() in forbidden_suffixes or lower_name.endswith(
            forbidden_archive_suffixes
        ):
            fail(f"generated or review artifact in source tree: {relative}")

    retired = "hw" + "scope"
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file() or path.name == "SHA256SUMS":
            continue
        try:
            content = read(path)
        except UnicodeDecodeError:
            continue
        if retired.lower() in content.lower() or retired.lower() in path.as_posix().lower():
            fail(f"retired product identity remains in {path.relative_to(ROOT)}")


def main() -> int:
    check_checksums()
    check_workspace()
    check_metadata()
    check_architecture_boundaries()
    check_modules()
    check_css()
    check_makefile()
    check_package_hygiene()

    if failures:
        for message in failures:
            print(f"source check: {message}", file=sys.stderr)
        return 1
    print("Source checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
