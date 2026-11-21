#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "Usage: $0 <version> <sha256> [output_dir]" >&2
  exit 1
fi

RAW_VERSION="$1"
SHA256="$2"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${3:-${ROOT_DIR}/packaging/aur/squad-air-bin}"

VERSION="${RAW_VERSION#v}"
ARCH_VER="${VERSION//-/_}"

mkdir -p "${OUT_DIR}"

cat > "${OUT_DIR}/PKGBUILD" <<EOF
# Maintainer: SquadAir Contributors <support@squadair.app>
pkgname=squad-air-bin
pkgver=${ARCH_VER}
pkgrel=1
pkgdesc="Ultra-lightweight, zero-knowledge P2P voice chat and 1080p60 screen sharing built in pure Rust"
arch=('x86_64')
url="https://squadair.app/"
license=('AGPL-3.0-only')
depends=(
  'alsa-lib'
  'gtk3'
  'libx11'
  'libxcb'
  'libxkbcommon'
  'libayatana-appindicator'
  'opus'
)
provides=('squad-air')
conflicts=('squad-air')
source=("squad-air-\${pkgver}-1-x86_64.pkg.tar.zst::https://github.com/alivergg/squad-air/releases/download/v${VERSION}/squad-air-${VERSION}-1-x86_64.pkg.tar.zst")
sha256sums=('${SHA256}')

package() {
  cp -a "\${srcdir}/usr" "\${pkgdir}/"
  install -dm755 "\${pkgdir}/usr/share/licenses/\${pkgname}"
  ln -sf ../squad-air/LICENSE "\${pkgdir}/usr/share/licenses/\${pkgname}/LICENSE"
}
EOF

cat > "${OUT_DIR}/.SRCINFO" <<EOF
pkgbase = squad-air-bin
	pkgdesc = Ultra-lightweight, zero-knowledge P2P voice chat and 1080p60 screen sharing built in pure Rust
	pkgver = ${ARCH_VER}
	pkgrel = 1
	url = https://squadair.app/
	arch = x86_64
	license = AGPL-3.0-only
	depends = alsa-lib
	depends = gtk3
	depends = libx11
	depends = libxcb
	depends = libxkbcommon
	depends = libayatana-appindicator
	depends = opus
	provides = squad-air
	conflicts = squad-air
	source = squad-air-${ARCH_VER}-1-x86_64.pkg.tar.zst::https://github.com/alivergg/squad-air/releases/download/v${VERSION}/squad-air-${VERSION}-1-x86_64.pkg.tar.zst
	sha256sums = ${SHA256}

pkgname = squad-air-bin
EOF
