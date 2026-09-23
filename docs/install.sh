#!/bin/sh
# Bootstrap Briefcase from Cargo; IAM sign-in is separate.
set -eu
case "$(uname -s)" in
  Darwin|Linux) ;;
  *) printf '%s\n' 'This installer supports macOS and Linux.' >&2; exit 1 ;;
esac
: "${HOME:?HOME must name your home directory}"
BRIEFCASE_RUST_VERSION=1.98.0
export RUSTUP_TOOLCHAIN="$BRIEFCASE_RUST_VERSION"
BRIEFCASE_INSTALL_ROOT=${BRIEFCASE_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}
if ! command -v rustup >/dev/null 2>&1; then
  command -v curl >/dev/null 2>&1 || { printf '%s\n' 'Install curl, then rerun this installer.' >&2; exit 1; }
  installer=$(mktemp)
  trap 'rm -f "$installer"' EXIT HUP INT TERM
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$installer"
  sh "$installer" -y --profile minimal --default-toolchain "$BRIEFCASE_RUST_VERSION"
  rm -f "$installer"
  trap - EXIT HUP INT TERM
fi
export PATH="$BRIEFCASE_INSTALL_ROOT/bin:${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
rustup toolchain install "$BRIEFCASE_RUST_VERSION" --profile minimal
if [ -n "${BRIEFCASE_INSTALL_SOURCE:-}" ]; then
  cargo "+$BRIEFCASE_RUST_VERSION" install --root "$BRIEFCASE_INSTALL_ROOT" --locked --path "$BRIEFCASE_INSTALL_SOURCE" --bin briefcase --force
elif [ -n "${BRIEFCASE_INSTALL_VERSION:-}" ]; then
  cargo "+$BRIEFCASE_RUST_VERSION" install briefcase-cli --root "$BRIEFCASE_INSTALL_ROOT" --locked --version "=$BRIEFCASE_INSTALL_VERSION" --bin briefcase --force
else
  cargo "+$BRIEFCASE_RUST_VERSION" install briefcase-cli --root "$BRIEFCASE_INSTALL_ROOT" --locked --bin briefcase --force
fi
printf '%s\n' 'Briefcase is installed. Manage future updates through Honeycomb.' 'Next: briefcase iam --json' 'Then: briefcase login <IAM-short-lived-token>' 'Offline documentation: briefcase docs cli'
