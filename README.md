# HWall

HWall is a read-only Linux hardware monitor and inventory application written in Rust. It provides native GTK 4 and terminal interfaces, with a hierarchical sensor view inspired by HWiNFO64. HWall reports information exposed by Linux, firmware, hardware drivers, and installed read-only helper tools.<br>

## History

I originally started this project for myself because I was tired of the lack of HWiNFO64 style sensor monitoring software for Linux, something that could show me everything I wanted to see in a simple table view, just like in Windows. It began as a personal tool, but once it became good enough in my opinion, I decided to release it for everyone, since I'm not probably the only one missing HWiNFO64 from Windows.

Most of the project was written with the help of AI. If you are not comfortable using AI-assisted software, you are under no obligation to use it. After version 1.0.4 binary packages are built using a GitHub CI/CD pipeline you can review or you can review the complete source and build it yourself.

## Screenshots

<img width="900" alt="image" src="https://github.com/user-attachments/assets/96a8169a-678d-4698-bd3c-90e0193d8d3f" />
<br>
<img width="450" alt="image" src="https://github.com/user-attachments/assets/020ccaeb-3ce4-494c-a99e-04ebcd48b4e6" />

## Features

- Live temperatures, fan speeds, voltages, utilization, clocks, storage activity, network rates, and available power readings
- APERF/MPERF-derived average and per-logical-CPU effective clocks on supported x86 systems
- Hierarchical **Sensors** and **Hardware** views in the GTK application
- System, light, and dark GTK theme preferences
- Switchable **Mixed**, **Sensors**, and **Hardware** views in the interactive terminal monitor
- Current, minimum, maximum, average, and sample counts for each sensor in its Details window
- Clear stale, unavailable, and offline states when a live reading can no longer be refreshed
- Configurable bounded in-memory sensor history with interactive charts, hover inspection, zooming, panning, selectable time ranges, timestamp inspection, and Save As export to CSV or JSON Lines
- Per-sensor warning and critical alerts with duration, hysteresis, cooldown, and desktop notifications
- Hardware inventory for processors, memory, graphics, storage, network, Bluetooth, USB, PCI, batteries, firmware, and supported security devices
- Optional SMART and NVMe health information
- Human-readable reports, filtered sensor output, JSON export, JSON Lines streaming, and an interactive terminal monitor
- HTTP JSON API: Serve the latest sensor, mixed, or hardware snapshot over HTTP with a configurable address and refresh interval

## Hardware support

Hardware support depends on the interfaces exposed by the running kernel and the installed drivers.

### Motherboard sensor drivers

Motherboard voltages, fan speeds, and board temperatures are available only when the appropriate Linux hwmon driver is loaded. HWall reads sensors already exposed under `/sys/class/hwmon`; it does not load kernel modules or request administrator privileges.

The table below provides examples of common sensor drivers; it is not a complete list.
| Module | Typical hardware | Common readings |
|---|---|---|
| `nct6775` | Modern Nuvoton Super-I/O chips | Voltages, temperatures, fan RPM, PWM |
| `it87` | ITE Super-I/O chips | Voltages, temperatures, fan RPM, PWM |
| `asus-ec-sensors` | Supported ASUS motherboards | board, chipset, and VRM temperatures, extra fans, some voltage/current readings |
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

### Optional runtime helpers

HWall can operate from Linux kernel interfaces alone, but installing the applicable runtime helpers is strongly recommended. Some hardware information and telemetry are unavailable without the helper that provides them. HWall detects available helpers automatically and skips integrations that are unavailable.

| Helper | Purpose |
|---|---|
| `lm-sensors` | **Strongly recommended.** Supplies configured sensor labels, motherboard-specific scaling, and improved interpretation of hwmon channels. HWall still reads ordinary hwmon sensors directly from sysfs. |
| `hwdata` | Provides readable PCI and USB vendor/device names through local ID databases where available. |
| BlueZ (`bluetoothctl`) | Adds currently connected Bluetooth devices, including device type and battery level when BlueZ reports them. |
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

Running `hwall-cli` in a terminal opens the interactive (hwall-cli watch) mixed view. Use Tab or Shift-Tab to cycle between Mixed, Sensors, and Hardware, or press 1, 2, or 3 to select a view directly. Use `hwall-cli --help` or `hwall-cli <subcommand> --help` for the complete option list. When output is redirected, it prints one mixed report instead. Explicit commands remain available:

```bash
hwall-cli
hwall-cli report
hwall-cli sensors
hwall-cli watch --view sensors
hwall-cli watch --view hardware
hwall-cli export --pretty
hwall-cli serve --listen 127.0.0.1:8765 --interval 1s
hwall-cli serve --view mixed
hwall-cli serve --view hardware
```

Examples:

```bash
hwall-cli sensors --class cpu
hwall-cli sensors --class cpu --kind effective-clock
hwall-cli sensors --device nvme --format json

hwall-cli watch --interval 500ms
hwall-cli watch --view hardware
hwall-cli watch --jsonl --interval 1s

hwall-cli --health report
hwall-cli --no-helpers report

hwall-cli serve
curl http://127.0.0.1:8765/
```

### HTTP API (serve)
The HTTP server exposes one JSON document at `/`. Its default `sensors` view returns a flat `sensors` array for dashboards and other integrations. Use `--view mixed` for the complete snapshot or `--view hardware` for inventory without sensor arrays.
```bash
hwall-cli serve
curl http://127.0.0.1:8765/
```

## Additional notes

- Sampling intervals are best-effort. External collectors such as sensors and nvidia-smi may take longer than the configured interval, especially at 200 ms. Individual sample intervals may vary because collection time and system scheduling vary, but HWall uses deadline-based timing so the average sampling interval stays close to the configured value whenever the system can sustain it. HWall does not queue delayed samples and instead runs at the fastest sustainable cadence. Very short sampling intervals combined with long history retention can consume substantial CPU and memory.
- When a live collector temporarily fails, HWall keeps the last known value visible, dims it, and marks it **Stale** with the time of its last successful update. When collection succeeds but a previously known sensor is no longer present, HWall marks it Unavailable until the next full hardware rediscovery confirms whether it has been removed.
- On supported x86 systems, HWall can show APERF/MPERF-derived effective clocks, including idle time, under **Effective clocks**. They require two samples and may be unavailable because of hardware, kernel, virtualization, perf permissions, or open-file limits.
- CPU package power represents the overall processor-package domain. CPU cores power represents only the execution-core domain and must not be added to package power. Per-core power is shown only when the processor and kernel provide physical-core energy counters.
- In the GTK settings, **Theme** can follow the system preference or force light or dark Adwaita styling.
- **Show identifying information** enables available serial numbers, UUIDs, WWNs, MAC addresses, and related identifiers, then performs a hardware rediscovery.
- **Keep sensor history** controls both the global in-memory retention period and the default chart range for newly opened sensor Details windows. The default is 1 minute; the shared limit is 24 hours.
- Development of HWall has been assisted by AI tools.

### Hardware monitoring notice

HWall reads kernel-exposed sensor values through interfaces such as hwmon and does not directly access embedded-controller registers. Some kernel drivers may query underlying firmware or hardware when those sensor attributes are read. For example, the Linux ASUS EC sensor driver reads embedded-controller registers while coordinating access through an ACPI mutex.

Hardware and firmware implementations vary. Avoid running multiple low-level hardware-monitoring applications simultaneously. If monitoring causes unusual latency, erratic readings, freezes, or other instability, stop HWall and other monitoring tools before troubleshooting.

HWall cannot guarantee the accuracy of reported values or compatibility with every system and is used at your own risk.