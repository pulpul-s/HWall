# HWall

HWall is a read-only Linux hardware monitor and inventory application written in Rust. It provides native GTK 4 and terminal interfaces, with a hierarchical sensor view inspired by HWiNFO64. HWall reports information exposed by Linux, firmware, hardware drivers, and installed read-only helper tools.<br>
<img width="900" alt="image" src="https://github.com/user-attachments/assets/ecdfd7ad-49da-441c-bb73-b100fd43f9a2" /><br>
<img width="600" alt="image" src="https://github.com/user-attachments/assets/cd4ed69a-fdb3-4bad-a4fd-be9423347e33" />


## Features

- Live temperatures, fan speeds, voltages, utilization, clocks, storage activity, network rates, and available power readings
- Hierarchical **Sensors** and **Hardware** views in the GTK application
- Switchable **Mixed**, **Sensors**, and **Hardware** views in the interactive terminal monitor
- Current, minimum, maximum, and average values for the active monitoring session
- Configurable bounded in-memory sensor history with mouse zoom, panning, timestamp inspection, and CSV or JSON Lines export
- Per-sensor warning and critical alerts with duration, hysteresis, cooldown, and desktop notifications
- Hardware inventory for processors, memory, graphics, storage, network, USB, PCI, batteries, firmware, and supported security devices
- Optional SMART and NVMe health information
- Human-readable reports, filtered sensor output, JSON export, JSON Lines streaming, and an interactive terminal monitor

Hardware support depends on the interfaces exposed by the running kernel and the installed drivers. Missing readings are omitted rather than guessed.

## Requirements

- Rust 1.92 or newer
- GTK 4.8 or newer development files for the graphical application
- `pkg-config`
- A standard C build toolchain

### Runtime helpers

HWall can operate from Linux kernel interfaces alone, but installing the applicable runtime helpers is strongly recommended. Some hardware information and telemetry are unavailable without the helper that provides them. HWall detects available helpers automatically and skips integrations that are unavailable.

| Helper | Purpose |
|---|---|
| `lm-sensors` | **Strongly recommended.** Supplies configured sensor labels, motherboard-specific scaling, and improved interpretation of hwmon channels. HWall still reads ordinary hwmon sensors directly from sysfs. |
| `hwdata` | Provides readable PCI vendor and device names through the local `pci.ids` database. |
| `ethtool` | Adds network driver and firmware information. |
| `smartmontools` | Required for detailed SMART health and lifetime information on supported SATA, SAS, and related drives. |
| `nvme-cli` | Required for detailed NVMe health warnings and lifetime counters. |
| NVIDIA utilities (`nvidia-smi`) | Required for complete NVIDIA telemetry. Package names vary by distribution and driver. |
| `dmidecode` | Recommended for detailed motherboard, firmware, TPM, CPU-socket, and memory-module inventory. **Access may require administrator privileges.** It is used only for optional discovery enrichment, not normal live polling; when access is unavailable, HWall skips this enrichment. |

For the most complete output, install the helpers relevant to the hardware in the system. Missing helpers do not prevent HWall from starting; only the affected information is omitted.

### Example build dependencies:

**Arch Linux**

```bash
sudo pacman -S --needed rust gtk4 pkgconf base-devel
```

**Debian or Ubuntu**

```bash
sudo apt install cargo rustc libgtk-4-dev pkg-config build-essential
```

**Fedora**
```bash
sudo dnf install rust cargo gtk4-devel pkgconf-pkg-config
```


## Building

Build the complete workspace and install:
```bash
make install
```

Build the complete workspace without installing:

```bash
cargo build --locked --workspace --release
```

The binaries are written to:

```text
target/release/hwall
target/release/hwall-cli
```

Build only the command-line application without GTK dependencies:

```bash
cargo build --locked -p hwall-cli --release
```

Build the GUI without tray integration:

```bash
cargo build --locked -p hwall-gui --release --no-default-features
```

## Installing

Install both binaries together with the desktop entry, application icon, and AppStream metadata:

```bash
sudo make install
```

The default prefix is `/usr/local`. `PREFIX` and `DESTDIR` can be overridden for staged installations; `make install-gui` and `make install-cli` install only the selected application. The committed `Cargo.lock` is used for reproducible builds. `make install` performs one workspace release build when either binary is missing or older than the source, and otherwise installs the existing binaries without invoking Cargo. Use `make clean` to remove build artifacts.

## Running

Start the GTK application:

```bash
hwall
```

Start the interactive CLI application:

```bash
hwall-cli
```

Use `--no-helpers` for sysfs-only collection, `--sensitive` to include identifying values such as serial numbers and MAC addresses, and `--health` to request slower SMART and NVMe health collection.

## CLI usage

Running `hwall-cli` in a terminal opens the interactive mixed view. When output is redirected, it prints one mixed report instead. Explicit commands remain available:

```bash
hwall-cli
hwall-cli report
hwall-cli sensors
hwall-cli watch --view sensors
hwall-cli watch --view hardware
hwall-cli export --pretty
```

Examples:

```bash
hwall-cli sensors --class cpu
hwall-cli sensors --device nvme --format json
hwall-cli watch --interval 500ms
hwall-cli watch --view sensors
hwall-cli watch --jsonl
hwall-cli --health report
hwall-cli --no-helpers report
```

The terminal interface starts in the mixed view. Use Tab or Shift-Tab to cycle between Mixed, Sensors, and Hardware, or press 1, 2, or 3 to select a view directly. Use `hwall-cli --help` or `hwall-cli <subcommand> --help` for the complete option list.

In the GTK settings, **Show identifying information** enables available serial numbers, UUIDs, WWNs, MAC addresses, and related identifiers after an automatic hardware rediscovery. **Keep sensor history** controls both the global in-memory retention period and the default chart range for newly opened sensor Details windows. The default is 1 minute; the shared limit is 24 hours.

