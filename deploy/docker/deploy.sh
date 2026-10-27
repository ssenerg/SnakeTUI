#!/bin/sh
# Deploys SnakeTUI on this server: pulls IMAGE@DIGEST and restarts the game
# with it. This script's directory holds docker-compose.yml, and .env if
# its settings change. If the game does not come up healthy, the image that
# ran before comes back and the script fails.
#
#   deploy.sh IMAGE [DIGEST]   e.g. deploy.sh ghcr.io/OWNER/game sha256:...
#
# CI runs it over SSH with a key that may run nothing else, with IMAGE set
# in authorized_keys and DIGEST sent by CI (see the README):
#
#   restrict,command="/opt/apps/snaketui/deploy.sh ghcr.io/OWNER/game" ssh-ed25519 ...
#
# The old game lets rounds in play finish before it stops, so this can take
# a few minutes. What it prints may end up in public CI logs, so the game's
# own logs, which name players and their addresses, go to the journal:
# journalctl -t snaketui-deploy
set -eu
cd "$(dirname "$0")"

fail() {
	echo "deploy: $*" >&2
	exit 1
}

image=${1:-}
digest=${2:-${SSH_ORIGINAL_COMMAND:-}}
[ -n "$image" ] || fail "usage: deploy.sh IMAGE [DIGEST]"
# The caller chooses the digest of an image in IMAGE, and nothing else.
hex=${digest#sha256:}
case $hex in
*[!0-9a-f]*) hex= ;;
esac
if [ "$digest" != "sha256:$hex" ] || [ ${#hex} -ne 64 ]; then
	fail "the digest must be sha256: and 64 lowercase hexadecimal digits"
fi

compose() { docker compose --file docker-compose.yml "$@"; }
journal() { logger -t snaketui-deploy; }

docker pull --quiet "$image@$digest" >/dev/null
# The image the game runs now stays at hand as snaketui:previous.
running=$(docker inspect --format '{{.Image}}' "$(compose ps --all --quiet snaketui 2>/dev/null)" 2>/dev/null || true)
if [ -n "$running" ]; then
	docker image tag "$running" snaketui:previous
fi
docker image tag "$image@$digest" snaketui:latest
docker image rm "$image@$digest" >/dev/null 2>&1 || true

# The old game drains first (up to its stop_grace_period), then the new one
# must pass its health check.
started() {
	compose up --detach --no-build --force-recreate --wait --wait-timeout 300 snaketui
}

if started; then
	# Images replaced by earlier deploys are not needed any more.
	docker image prune --force --filter label=org.opencontainers.image.title=snaketui >/dev/null
	echo "deploy: snaketui $(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' snaketui:latest) is running"
	exit 0
fi

compose logs --no-color --tail 100 snaketui 2>&1 | journal || true
logs="Its logs are on the server: journalctl -t snaketui-deploy"
[ -n "$running" ] || fail "snaketui did not come up healthy. $logs"
docker image tag snaketui:previous snaketui:latest
if started >/dev/null 2>&1; then
	fail "snaketui did not come up healthy, so the image that ran before is back. $logs"
fi
fail "snaketui did not come up healthy, and neither did the image that ran before. $logs"
