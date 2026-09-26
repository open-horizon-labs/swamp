#!/bin/zsh
# A throwaway repo plus the full range of Docker objects the tool has to
# reason about, so every branch can be exercised by hand:
#
#   joined by compose label      an image and two volumes carrying
#                                com.docker.compose.project
#   joined by image.source       an image labelled with the repo's remote
#   unowned                      an image with no join evidence at all
#   dangling                     an untagged image, built with no tag
#   in use                       a stopped container holding an image, so
#                                removal is refused by the daemon, in its
#                                own words
#   volume with contents         written by a container, so the bytes are
#                                real and the "exists nowhere else"
#                                warning has something behind it
#   build cache                  RUN layers, which the tool reports and
#                                refuses to remove per-entry
#
#   scripts/docker-fixture.sh up   [root]   # default root: ~/src
#   scripts/docker-fixture.sh down [root]
#
# Everything created is named `slop-fixture*`, so `down` can find it.
set -euo pipefail
action="${1:-up}"
root="${2:-$HOME/src}"
name="slop-fixture"
repo="$root/$name"
base="busybox:1.37.0"

banner() { print -P "%F{cyan}$1%f"; }

case "$action" in
up)
  if ! docker image inspect "$base" > /dev/null 2>&1; then
    banner "pulling $base (6MB, once)"
    docker pull -q "$base" > /dev/null
  fi

  mkdir -p "$repo"
  cd "$repo"
  if [ ! -d .git ]; then
    git init -q
    git config user.email fixture@example.com
    git config user.name fixture
    # A remote, so the image.source join has something to match.
    git remote add origin https://github.com/example/slop-fixture.git
  fi

  cat > compose.yaml <<YAML
name: $name
services:
  app:
    image: $name-app:latest
    build:
      context: .
      dockerfile: Dockerfile.app
    volumes:
      - app-data:/data
  worker:
    image: $name-worker:latest
    build:
      context: .
      dockerfile: Dockerfile.worker
volumes:
  app-data:
  scratch-data:
YAML

  # An image with real layers and build cache behind it.
  cat > Dockerfile.app <<DOCKER
FROM $base
RUN mkdir -p /opt/app && dd if=/dev/zero of=/opt/app/blob bs=1048576 count=40 2>/dev/null
RUN echo "second layer" > /opt/app/marker
DOCKER
  # A second image, joined only by its source label.
  cat > Dockerfile.worker <<DOCKER
FROM $base
RUN dd if=/dev/zero of=/worker.blob bs=1048576 count=16 2>/dev/null
DOCKER
  # A third, with no evidence of any kind: this one must read as unowned.
  cat > Dockerfile.orphan <<DOCKER
FROM $base
RUN dd if=/dev/zero of=/orphan.blob bs=1048576 count=24 2>/dev/null
DOCKER

  printf 'payload.bin\nDockerfile.dangling\n.dangling-id\n' > .gitignore
  dd if=/dev/urandom of=payload.bin bs=1048576 count=18 status=none
  git add -A && git commit -qm "docker fixture" || true

  banner "building three images"
  docker build -q -f Dockerfile.app \
    --label com.docker.compose.project="$name" \
    -t "${name}-app:latest" . > /dev/null
  docker build -q -f Dockerfile.worker \
    --label org.opencontainers.image.source=https://github.com/example/slop-fixture \
    -t "${name}-worker:latest" . > /dev/null
  docker build -q -f Dockerfile.orphan -t "${name}-orphan:latest" . > /dev/null

  # A dangling image. Rebuilding a tagged image does not produce one on
  # a daemon using the containerd image store (moving the tag drops the
  # old record instead of leaving it untagged), so build one with no tag
  # at all: that is untagged on every store. The content changes each run
  # so the build cache cannot hand back an image that already exists.
  cat > Dockerfile.dangling <<DOCKER
FROM $base
RUN dd if=/dev/zero of=/dangling.blob bs=1048576 count=12 2>/dev/null
RUN echo dangling-$(date +%s) > /marker
DOCKER
  # Remove the one the previous `up` built, so re-running leaves one
  # dangling image rather than a growing pile. Only ever this fixture's
  # own id: a blanket `docker image prune` would take the user's.
  if [ -f .dangling-id ]; then
    docker image rm -f "$(cat .dangling-id)" > /dev/null 2>&1 || true
  fi
  docker build -q -f Dockerfile.dangling . | tail -1 | sed 's/^sha256://' > .dangling-id

  banner "creating volumes, one with contents"
  docker volume create --label com.docker.compose.project="$name" "${name}-app-data" > /dev/null
  docker volume create --label com.docker.compose.project="$name" "${name}-scratch-data" > /dev/null
  docker run --rm -v "${name}-app-data":/data "$base" \
    sh -c 'dd if=/dev/zero of=/data/blob bs=1048576 count=32 2>/dev/null' > /dev/null

  banner "leaving a stopped container, so one image cannot be removed"
  docker rm -f "${name}-held" > /dev/null 2>&1 || true
  docker create --name "${name}-held" "${name}-worker:latest" true > /dev/null

  print ""
  banner "up: $repo"
  docker images --format '  image   {{.Repository}}:{{.Tag}} {{.Size}}' | grep -E "$name|<none>" || true
  docker volume ls --format '  volume  {{.Name}}' | grep "$name" || true
  print "  image   <none> (dangling, $(cat .dangling-id | cut -c1-12))"
  print "  container $name-held (stopped, holds ${name}-worker:latest)"
  print ""
  print "Try:"
  print "  swamp report $root --project $name           # the join"
  print "  swamp report $root --view docker             # joined and unowned together"
  print "  swamp ui $root                               # 5 for docker, Space to mark, A for all"
  print ""
  print "The worker image is held by a container: removing it must be refused"
  print "in the daemon's own words. The orphan image belongs to no project."
  ;;
down)
  docker rm -f "${name}-held" > /dev/null 2>&1 || true
  for v in "${name}-app-data" "${name}-scratch-data"; do
    docker volume rm -f "$v" > /dev/null 2>&1 || true
  done
  for i in "${name}-app:latest" "${name}-worker:latest" "${name}-orphan:latest"; do
    docker image rm -f "$i" > /dev/null 2>&1 || true
  done
  if [ -f "$repo/.dangling-id" ]; then
    docker image rm -f "$(cat "$repo/.dangling-id")" > /dev/null 2>&1 || true
  fi
  rm -rf "$repo"
  # The earlier minimal probe, if it is still around.
  rm -rf "$root/slop-docker-probe"
  docker volume rm -f probe-data > /dev/null 2>&1 || true
  echo "down: removed the fixture's images, volumes, container and $repo"
  ;;
*)
  echo "usage: $0 up|down [root]" >&2
  exit 2
  ;;
esac
