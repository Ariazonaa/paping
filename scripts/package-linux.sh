#!/bin/sh
# Builds a .deb and an .rpm from a release staging directory.
#
#     scripts/package-linux.sh TARGET STAGING_DIR OUT_DIR
#
# STAGING_DIR holds the static `paping` binary plus LICENSE, NOTICE,
# THIRD-PARTY-LICENSES.txt, README.md and CHANGELOG.md, as the release workflow
# lays them out. TARGET is the Rust target triple (x86_64-/aarch64-unknown-linux-musl).
# A package is skipped when its tool (dpkg-deb, rpmbuild) is not installed.
set -eu

target="$1"
staging="$2"
out="$3"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
case "$target" in
  x86_64-*) deb_arch=amd64 rpm_arch=x86_64 ;;
  aarch64-*) deb_arch=arm64 rpm_arch=aarch64 ;;
  *) echo "unsupported target: $target" >&2; exit 1 ;;
esac
maintainer="Ariazonaa <96840802+Ariazonaa@users.noreply.github.com>"
summary="TCP port ping with TLS and HTTP timing over IPv4 and IPv6"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# A text file as a Debian copyright field body: indented, empty lines as " .", no CR.
as_field() {
  tr -d '\r' < "$1" | sed 's/^$/./; s/^/ /'
}

# --- .deb -------------------------------------------------------------------
build_deb() {
root="$work/deb"
install -Dm755 "$staging/paping" "$root/usr/bin/paping"
doc="$root/usr/share/doc/paping"
install -Dm644 "$staging/README.md" "$doc/README.md"
install -Dm644 "$staging/CHANGELOG.md" "$doc/CHANGELOG.md"
# Debian keeps all license texts of a package in one copyright file.
{
  echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
  echo "Upstream-Name: paping"
  echo "Source: https://github.com/Ariazonaa/paping"
  echo
  echo "Files: *"
  echo "Copyright: 2026 Ariazonaa"
  echo "License: MIT"
  echo
  as_field "$staging/LICENSE"
  echo
  echo "Comment:"
  as_field "$staging/NOTICE"
  echo " ."
  as_field "$staging/THIRD-PARTY-LICENSES.txt"
} > "$doc/copyright"
chmod 644 "$doc/copyright"
size_kb="$(du -sk "$root/usr" | cut -f1)"
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<EOF
Package: paping
Version: $version
Architecture: $deb_arch
Maintainer: $maintainer
Installed-Size: $size_kb
Section: net
Priority: optional
Homepage: https://github.com/Ariazonaa/paping
Description: $summary
 paping connects to a host and port over and over and shows how long each
 connect takes, the way ping does for ICMP. It checks IPv4 and IPv6 side by
 side, can time the TLS handshake and an HTTP request, and writes JSON or CSV
 for scripts. A drop-in replacement for paping 1.5.5.
EOF
dpkg-deb --root-owner-group --build "$root" "$out/paping_${version}_${deb_arch}.deb" >/dev/null
echo "built $out/paping_${version}_${deb_arch}.deb"
}
if command -v dpkg-deb >/dev/null 2>&1; then
  build_deb
else
  echo "dpkg-deb not found, skipping .deb" >&2
fi

# --- .rpm -------------------------------------------------------------------
if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "rpmbuild not found, skipping .rpm" >&2
  exit 0
fi
top="$work/rpm"
mkdir -p "$top/SOURCES" "$top/SPECS"
cp "$staging/paping" "$staging/LICENSE" "$staging/NOTICE" "$staging/THIRD-PARTY-LICENSES.txt" \
  "$staging/README.md" "$staging/CHANGELOG.md" "$top/SOURCES/"
cat > "$top/SPECS/paping.spec" <<EOF
Name:           paping
Version:        $version
Release:        1
Summary:        $summary
License:        MIT
URL:            https://github.com/Ariazonaa/paping
Packager:       $maintainer

# A prebuilt static binary: nothing to compile, strip or extract debug info from.
%global debug_package %{nil}
%global __os_install_post %{nil}
# The history lives in CHANGELOG.md; take the build date instead of a %%changelog entry.
%global source_date_epoch_from_changelog 0

%description
paping connects to a host and port over and over and shows how long each
connect takes, the way ping does for ICMP. It checks IPv4 and IPv6 side by
side, can time the TLS handshake and an HTTP request, and writes JSON or CSV
for scripts. A drop-in replacement for paping 1.5.5.

%install
install -Dm755 %{_sourcedir}/paping %{buildroot}%{_bindir}/paping
install -Dm644 %{_sourcedir}/LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
install -Dm644 %{_sourcedir}/NOTICE %{buildroot}%{_licensedir}/%{name}/NOTICE
install -Dm644 %{_sourcedir}/THIRD-PARTY-LICENSES.txt %{buildroot}%{_licensedir}/%{name}/THIRD-PARTY-LICENSES.txt
install -Dm644 %{_sourcedir}/README.md %{buildroot}%{_docdir}/%{name}/README.md
install -Dm644 %{_sourcedir}/CHANGELOG.md %{buildroot}%{_docdir}/%{name}/CHANGELOG.md

%files
%{_bindir}/paping
%{_licensedir}/%{name}
%{_docdir}/%{name}
EOF
rpmbuild --quiet -bb --target "$rpm_arch" --define "_topdir $top" "$top/SPECS/paping.spec"
cp "$top/RPMS/$rpm_arch/paping-$version-1.$rpm_arch.rpm" "$out/"
echo "built $out/paping-$version-1.$rpm_arch.rpm"
