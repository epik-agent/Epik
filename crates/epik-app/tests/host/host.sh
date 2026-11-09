#!/bin/sh
# A throwaway remote host in a Docker container, reached over real ssh.
#
#   host.sh up >> ~/.ssh/config    builds and starts it as `devbox`
#   host.sh down                   stops and removes it
#
# `up` prints only the ~/.ssh/config block on stdout; progress goes to
# stderr. The host runs this checkout's headless app, built for Linux into
# target/host and mounted in, with `gh` and Node installed by its own
# `epik-app setup install`. Its sign-ins are left to do by hand.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
out="$repo/target/host"
name=epik-host
port=${EPIK_HOST_PORT:-2222}
key="$out/id_ed25519"

say() { echo "host.sh: $*" >&2; }

# The headless app, for Linux: here if this is Linux, else in Rust's image.
build_app() {
    say "building the headless app for Linux"
    if [ "$(uname -s)" = Linux ]; then
        CARGO_TARGET_DIR="$out" cargo build --release --locked -p epik-app --no-default-features >&2
    else
        docker run --rm -v "$repo:/src" -w /src -e CARGO_TARGET_DIR=/src/target/host \
            -v epik-host-cargo:/usr/local/cargo/registry \
            rust:1 cargo build --release --locked -p epik-app --no-default-features >&2
    fi
}

host_ssh() {
    ssh -F /dev/null -i "$key" -p "$port" -o BatchMode=yes -o IdentitiesOnly=yes \
        -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
        epik@127.0.0.1 "$@"
}

up() {
    build_app
    [ -f "$key" ] || ssh-keygen -q -t ed25519 -N '' -C epik-host -f "$key" >&2

    say "building the image"
    docker build -q -t "$name" --build-arg PUBLIC_KEY="$(cat "$key.pub")" "$here" >&2

    say "starting $name on port $port"
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -p "127.0.0.1:$port:22" \
        -v "$out/release/epik-app:/home/epik/.epik/bin/epik-app:ro" "$name" >/dev/null

    tries=0
    until host_ssh true 2>/dev/null; do
        tries=$((tries + 1))
        if [ "$tries" -ge 60 ]; then
            say "ssh to $name didn't answer"
            docker logs "$name" >&2
            exit 1
        fi
        sleep 1
    done

    say "installing gh and Node with Epik's installers"
    host_ssh .epik/bin/epik-app setup install gh >&2
    host_ssh .epik/bin/epik-app setup install node >&2

    cat <<CONFIG

Host devbox
    HostName 127.0.0.1
    Port $port
    User epik
    IdentityFile $key
    IdentitiesOnly yes
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
CONFIG
}

down() {
    docker rm -f "$name" >/dev/null
}

case "${1:-}" in
    up) up ;;
    down) down ;;
    *)
        echo "usage: host.sh up | down" >&2
        exit 2
        ;;
esac
