%global debug_package %{nil}

Name:           hwall
Version:        %{hwall_version}
Release:        1%{?dist}
Summary:        Linux hardware inventory and live sensor monitor
License:        MIT
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gtk4-devel >= 4.8
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
BuildRequires:  rust >= 1.92
Requires:       gtk4 >= 4.8
Recommends:     lm_sensors
Recommends:     hwdata
Recommends:     ethtool
Recommends:     smartmontools
Recommends:     nvme-cli
Recommends:     dmidecode
Recommends:     polkit

%description
HWall provides GTK 4 and terminal interfaces for hardware inventory,
live sensors, alerts, history, and storage health information.

%build
make release

%install
make DESTDIR=%{buildroot} PREFIX=%{_prefix} install

%files
%license LICENSE
%{_bindir}/hwall
%{_bindir}/hwall-cli
%{_datadir}/applications/io.github.hwall.HWall.desktop
%{_datadir}/metainfo/io.github.hwall.HWall.metainfo.xml
%{_datadir}/icons/hicolor/32x32/apps/io.github.hwall.HWall.png
%{_datadir}/icons/hicolor/48x48/apps/io.github.hwall.HWall.png
%{_datadir}/icons/hicolor/64x64/apps/io.github.hwall.HWall.png
%{_datadir}/icons/hicolor/scalable/apps/io.github.hwall.HWall.svg
