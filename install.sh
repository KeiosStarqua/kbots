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
cargo build --release --manifest-path "${repo_root}/gui/Cargo.toml"

printf 'Building the Ratatui terminal UI...\n'
cargo build --release --manifest-path "${repo_root}/tui/control-center/Cargo.toml"

printf 'Building the automation CLI...\n'
cargo build --release --manifest-path "${repo_root}/cli/Cargo.toml"

mkdir -p "${install_dir}"
install -m 755 \
  "${repo_root}/gui/target/release/kbots-control-center" \
  "${install_dir}/kbots-gui"
install -m 755 \
  "${repo_root}/tui/control-center/target/release/kbots-tui" \
  "${install_dir}/kbots-tui"
install -m 755 \
  "${repo_root}/cli/target/release/kbots" \
  "${install_dir}/kbots"

rm -f "${install_dir}/kbots-cli"

# Short alias: kb -> kbots
rm -f "${install_dir}/kb"
ln -s kbots "${install_dir}/kb"

desktop_dir="${XDG_DATA_HOME:-${HOME}/.local/share}/applications"
desktop_file="${desktop_dir}/kbots.desktop"
desktop_exec="${install_dir}/kbots-gui"
desktop_exec="${desktop_exec//\\/\\\\}"
desktop_exec="${desktop_exec//\"/\\\"}"
desktop_exec="${desktop_exec//\`/\\\`}"
desktop_exec="${desktop_exec//\$/\\\$}"
mkdir -p "${desktop_dir}"
while IFS= read -r line || [[ -n "${line}" ]]; do
  if [[ "${line}" == 'Exec=@KBOTS_GUI_EXEC@' ]]; then
    printf 'Exec="%s"\n' "${desktop_exec}"
  else
    printf '%s\n' "${line}"
  fi
done < "${repo_root}/gui/kbots.desktop.in" > "${desktop_file}"
chmod 644 "${desktop_file}"

printf '\nInstalled commands:\n  %s\n  %s (alias for kbots)\n  %s\n  %s\nDesktop launcher:\n  %s\n' \
  "${install_dir}/kbots" \
  "${install_dir}/kb" \
  "${install_dir}/kbots-gui" \
  "${install_dir}/kbots-tui" \
  "${desktop_file}"

unit_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
if [[ "${KBOTS_SKIP_DAEMON_UNIT:-0}" == "1" ]]; then
  printf '\nSkipping kbots-daemon user service (KBOTS_SKIP_DAEMON_UNIT=1).\n'
elif command -v systemctl >/dev/null 2>&1 && systemctl --user list-units >/dev/null 2>&1; then
  bun_path="$(command -v bun)"
  mkdir -p "${unit_dir}"
  while IFS= read -r line || [[ -n "${line}" ]]; do
    line="${line//@REPO_ROOT@/${repo_root}}"
    line="${line//@BUN@/${bun_path}}"
    printf '%s\n' "${line}"
  done < "${repo_root}/deploy/kbots-daemon.service" > "${unit_dir}/kbots-daemon.service"
  chmod 644 "${unit_dir}/kbots-daemon.service"
  systemctl --user daemon-reload
  systemctl --user enable --now kbots-daemon.service >/dev/null
  systemctl --user restart kbots-daemon.service
  # Type=simple reports active before the daemon binds the socket, so give it a
  # moment before returning (never fails the install).
  socket_path="${KBOTS_SOCKET:-${XDG_RUNTIME_DIR:-$HOME/.local/run}/kbots.sock}"
  for _ in $(seq 1 50); do
    [[ -S "${socket_path}" ]] && break
    sleep 0.2
  done
  printf '\nDeployed kbots-daemon user service (starts on login, auto-restarts): %s\n' "${unit_dir}/kbots-daemon.service"
else
  printf '\nsystemd user session unavailable; start the daemon manually with `bun run daemon`.\n'
fi

# Tag the deployed commit with a CalVer deploy tag so every deploy is
# traceable in git (git tag -l 'deploy/*'). Stable releases use SemVer
# tags (vX.Y.Z) instead; the daemon reports the recorded version, or the
# DAEMON_VERSION SemVer when no deploy version is recorded.
if command -v git >/dev/null 2>&1 && git -C "${repo_root}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  deploy_date="$(date +%Y.%m.%d)"
  deploy_n=1
  while git -C "${repo_root}" rev-parse -q --verify "refs/tags/deploy/${deploy_date}.${deploy_n}" >/dev/null 2>&1; do
    deploy_n=$((deploy_n + 1))
  done
  deploy_version="${deploy_date}.${deploy_n}"
  deploy_tag="deploy/${deploy_version}"
  if [[ -n "$(git -C "${repo_root}" status --porcelain)" ]]; then
    deploy_record="${deploy_version}+dirty"
    printf '\nWARNING: the working tree has uncommitted changes; the deployed code is not exactly tag %s.\n' "${deploy_tag}" >&2
  else
    deploy_record="${deploy_version}"
  fi
  if git -C "${repo_root}" tag -a "${deploy_tag}" -m "Deploy ${deploy_tag} ($(git -C "${repo_root}" rev-parse --short HEAD))"; then
    printf '%s\n' "${deploy_record}" > "${repo_root}/.deploy-version"
    printf 'Tagged deploy: %s (recorded in .deploy-version; `kb status` reports it)\n' "${deploy_tag}"
  else
    printf '\nWARNING: could not create deploy tag %s; daemon will report the SemVer constant.\n' "${deploy_tag}" >&2
  fi
else
  printf '\nNote: git not available or not a work tree; skipping deploy tag and .deploy-version.\n'
fi

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) printf 'Add %s to PATH to run these commands by name.\n' "${install_dir}" ;;
esac

printf '\nDeployed commands and the kbots-daemon user service (or manual `bun run daemon` without systemd).\n'
