# A remote host set up as the README's "Remote hosts" says, for tests:
# reached over ssh as the user `epik`, with the app's binary mounted at
# ~/.epik/bin/epik-app. `host.sh up` builds and runs it.
FROM ubuntu:24.04

# Node, which Epik installs, needs libatomic1, as the README says.
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server ca-certificates curl libatomic1 \
    && rm -rf /var/lib/apt/lists/*

# Keys only. A `*` password, unlike useradd's locked `!`, lets a key log in.
RUN useradd --create-home --shell /bin/bash epik \
    && usermod -p '*' epik \
    && mkdir -p /run/sshd \
    && printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\n' \
        > /etc/ssh/sshd_config.d/keys-only.conf

USER epik
WORKDIR /home/epik
RUN mkdir -p .epik/bin .ssh && chmod 700 .ssh \
    && curl -fsSL https://claude.ai/install.sh | bash
ARG PUBLIC_KEY
RUN echo "$PUBLIC_KEY" > .ssh/authorized_keys && chmod 600 .ssh/authorized_keys

# `ssh host command` runs with a fixed PATH, without ~/.local/bin.
USER root
RUN ln -s /home/epik/.local/bin/claude /usr/local/bin/claude

COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
