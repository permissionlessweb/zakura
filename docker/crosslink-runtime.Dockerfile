# syntax=docker/dockerfile:1
# Runtime wrapper around docker/ubuntu-package.Dockerfile --target artifact.
# bash is required for the Akash SDL (skip entrypoint.sh chown UID:GID).
# Build: docker build -f docker/ubuntu-package.Dockerfile --target artifact \
#          --build-arg FEATURES=crosslink -t zakura-bin:local .
#        docker build -f docker/crosslink-runtime.Dockerfile -t registry... .
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates bash curl adduser \
    && addgroup --gid 10001 zebra \
    && adduser --uid 10001 --gid 10001 --home /home/zebra --disabled-password --gecos "" zebra \
    && rm -rf /var/lib/apt/lists/*
COPY --from=zakura-bin:local /zakurad /usr/local/bin/zakurad
RUN chmod +x /usr/local/bin/zakurad
ENV HOME=/home/zebra FEATURES=crosslink
WORKDIR /home/zebra
USER root
# No entrypoint.sh — SDL execs /usr/local/bin/zakurad after writing toml.
ENTRYPOINT ["/bin/bash"]
CMD ["-lc", "exec /usr/local/bin/zakurad start"]
