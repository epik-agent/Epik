#!/bin/sh
# Runs the app as the README's systemd unit does, as `epik` from its home,
# and sshd. Under systemd, the unit would replace the line that starts it.
set -e
ssh-keygen -A
# shellcheck disable=SC2016 # $HOME is epik's, expanded by its shell.
su - epik -c 'exec "$HOME/.epik/bin/epik-app" --headless' &
exec /usr/sbin/sshd -D -e
