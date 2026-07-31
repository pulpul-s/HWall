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

## Hardware support

Hardware support depends on the interfaces exposed by the running kernel and the installed drivers. Missing readings are omitted rather than guessed.

### Motherboard sensor drivers

Motherboard voltages, fan speeds, and board temperatures are available only when the appropriate Linux hwmon driver is loaded. HWall reads sensors already exposed under `/sys/class/hwmon`; it does not load kernel modules or request administrator privileges.

| Module | Typical hardware | Common readings |
|---|---|---|
| `nct6775` | Modern Nuvoton Super-I/O chips | Voltages, temperatures, fan RPM, PWM |
| `it87` | ITE Super-I/O chips | Voltages, temperatures, fan RPM, PWM |
| `asus-ec-sensors` | Supported ASUS motherboards | Board, chipset and VRM temperatures, extra fans, some voltage/current readings |
| `asus_wmi_sensors` | Older supported ASUS boards | Voltages, temperatures, fans, current and water-cooling headers |
| `w83627ehf` | Older Winbond/Nuvoton chips | Voltages, temperatures and fans |
| `f71882fg` | Fintek Super-I/O chips | Voltages, temperatures and fans |

Available channels depend on the motherboard, monitoring chip, driver, and kernel version.

For example, many Nuvoton monitoring chips use the `nct6775` driver. Load a known driver for the current boot with:

```bash
sudo modprobe nct6775
```

### CPU power monitoring

On supported processors, HWall can derive CPU package, aggregate core-domain, per-core, DRAM, integrated GPU, and platform power from Linux powercap or perf energy counters.

On many modern Linux systems, perf energy counters are restricted to administrators by default. HWall checks each domain independently and omits readings it cannot access.

To allow ordinary users to access system-wide perf energy counters for the current boot:

```bash
sudo sysctl kernel.perf_event_paranoid=0
```

This permits local users to perform system-wide performance monitoring. Review the Linux kernel [`perf_event_paranoid` sysctl guide](https://docs.kernel.org/admin-guide/sysctl/kernel.html#perf-event-paranoid) and enable it only when that access is acceptable.

CPU package power represents the overall processor-package domain. CPU cores power represents only the execution-core domain and must not be added to package power. Per-core power is shown only when the processor and kernel provide physical-core energy counters.

### Optional runtime helpers

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

## Requirements

- Rust 1.92 or newer
- GTK 4.8 or newer development files for the graphical application
- `pkg-config`
- A standard C build toolchain

### Example build dependencies

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

Build the complete workspace:

```bash
make release
```

The binaries are written to:

```text
target/release/hwall
target/release/hwall-cli
```

Build only the GUI:

```bash
make release-gui
```

Build the GUI without tray integration:

```bash
cargo build --locked -p hwall-gui --release --no-default-features
```

Build only the command-line application without GTK dependencies:

```bash
make release-cli
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

