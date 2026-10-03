# Maintainer: ByteAtATime <hi@byteatatime.dev>
pkgname=pakajo
pkgver=0.1.0
pkgrel=1
pkgdesc='AUR helper and package manager front-end for Arch Linux, with a terminal CLI and a COSMIC GUI'
arch=('x86_64')
url='https://github.com/ByteAtATime/pakajo'
license=('GPL-3.0-only')
_tag=v0.1.0
source=("git+https://github.com/ByteAtATime/pakajo.git#tag=${_tag}")
sha256sums=('SKIP')

pkgver() {
  cd "$srcdir/pakajo"
  awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml
}

depends=(
  'bash'
  'coreutils'
  'git'
  'libglvnd'
  'libx11'
  'libxcursor'
  'libxkbcommon'
  'libxi'
  'libxrandr'
  'mesa'
  'pacman>=7.0.0'
  'polkit'
  'sudo'
  'systemd-libs'
  'vulkan-icd-loader'
  'wayland'
  'xorg-xwayland'
)

optdepends=(
  'bash-completion: bash tab completion'
  'less: pager for review output'
  'libnotify: desktop notifications'
  'xdg-utils: open links'
)

makedepends=(
  'git'
  'pkgconf'
  'rust>=1.85'
)

build() {
  cd "$srcdir/pakajo"
  cargo build --release --locked
}

check() {
  cd "$srcdir/pakajo"
  local pakajo=target/release/pakajo

  "$pakajo" --help >/dev/null
  "$pakajo" help search >/dev/null
  "$pakajo" completions bash >/dev/null
}

package() {
  cd "$srcdir/pakajo"
  local pakajo=target/release/pakajo

  install -Dm755 "$pakajo" "${pkgdir}/usr/bin/pakajo"

  install -Dm644 resources/pakajo.desktop \
    "${pkgdir}/usr/share/applications/pakajo.desktop"
  install -Dm644 resources/com.pakajo.policy \
    "${pkgdir}/usr/share/polkit-1/actions/com.pakajo.policy"

  "$pakajo" completions bash | install -Dm644 /dev/stdin \
    "${pkgdir}/usr/share/bash-completion/completions/pakajo"
  "$pakajo" completions zsh | install -Dm644 /dev/stdin \
    "${pkgdir}/usr/share/zsh/site-functions/_pakajo"
  "$pakajo" completions fish | install -Dm644 /dev/stdin \
    "${pkgdir}/usr/share/fish/vendor_completions.d/pakajo.fish"
}
