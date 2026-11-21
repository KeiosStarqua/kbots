# One command to deploy kbots from a Nix-managed host:
#   kbdeploy "what changed"
# Commits the checkout, tags the deploy (like install.sh), pushes, refreshes
# the home-manager flake input, switches, and shows the resulting status.
# Paths are overridable via KBOTS_REPO / HOME_MANAGER_DIR.
set -Eeuo pipefail

repo="${KBOTS_REPO:-$HOME/repos/mein/kbots}"
hm="${HOME_MANAGER_DIR:-$HOME/.config/home-manager}"
message="$*"
if [[ -z "$message" ]]; then
  message="kbots: deploy $(date -u +%Y-%m-%dT%H:%M:%S)"
fi

echo "==> commit $repo"
cd "$repo"
if [[ -n "$(git status --porcelain)" ]]; then
  git add -A
  git commit -m "$message"
else
  echo "    no local changes; deploying current HEAD"
fi

# CalVer deploy tag, per-day counter, same scheme as install.sh.
day="$(date +%Y.%m.%d)"
n=1
while git rev-parse -q --verify "refs/tags/deploy/${day}.${n}" >/dev/null; do
  n=$((n + 1))
done
tag="deploy/${day}.${n}"
git tag -a "$tag" -m "Deploy $tag ($(git rev-parse --short HEAD))"
echo "==> tagged $tag"

if git remote get-url origin >/dev/null 2>&1; then
  echo "==> push"
  git push origin HEAD --follow-tags
fi

echo "==> refresh the kbots flake input"
cd "$hm"
nix flake update kbots

echo "==> home-manager switch"
home-manager switch --flake "$hm#$(id -un)"

echo "==> kb status"
kb status
