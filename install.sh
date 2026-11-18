#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
install_dir="${KBOTS_INSTALL_DIR:-${HOME}/.local/bin}"

for command_name in bun cargo install mkdir; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "${command_name}" >&2
    exit 1
  fi
done

printf 'Installing Bun dependencies from the frozen lockfile...\n'
(cd "${repo_root}" && bun install --frozen-lockfile)
(cd "${repo_root}" && bun run tsc --noEmit)

printf 'Building the Slint desktop GUI...\n'
cargo build --release --manifest-path "${repo_root}/gui/control-center/Cargo.toml"

printf 'Building the Ratatui terminal UI...\n'
cargo build --release --manifest-path "${repo_root}/tui/control-center/Cargo.toml"

printf 'Building the automation CLI...\n'
cargo build --release --manifest-path "${repo_root}/cli/Cargo.toml"

mkdir -p "${install_dir}"
install -m 755 \
  "${repo_root}/gui/control-center/target/release/kbots-control-center" \
  "${install_dir}/kbots-gui"
install -m 755 \
  "${repo_root}/tui/control-center/target/release/kbots-tui" \
  "${install_dir}/kbots-tui"
install -m 755 \
  "${repo_root}/cli/target/release/kbots-cli" \
  "${install_dir}/kbots-cli"

printf '\nInstalled commands:\n  %s\n  %s\n  %s\n' \
  "${install_dir}/kbots-gui" \
  "${install_dir}/kbots-tui" \
  "${install_dir}/kbots-cli"

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) printf 'Add %s to PATH to run these commands by name.\n' "${install_dir}" ;;
esac

printf '\nThe Bun backend remains a source package; its daemon executable is not implemented yet.\n'
