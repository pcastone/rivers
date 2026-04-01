## Rivers runtime container
## Build: docker build --platform linux/amd64 -t rivers:latest -f docker/Dockerfile .
## Run:   docker run -p 8080:8080 -v ./my-bundle:/app/bundle rivers:latest

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    apt-get clean && rm -rf /var/lib/apt/lists/* && \
    groupadd -r rivers && useradd -r -g rivers -d /var/lib/rivers -s /sbin/nologin rivers && \
    mkdir -p /etc/rivers /var/lib/rivers /var/log/rivers && \
    chown -R rivers:rivers /var/lib/rivers /var/log/rivers

COPY target/x86_64-unknown-linux-gnu/ci/riversd /usr/bin/riversd
COPY target/x86_64-unknown-linux-gnu/ci/riversctl /usr/bin/riversctl
COPY packaging/config/riversd.toml /etc/rivers/riversd.toml

USER rivers
WORKDIR /var/lib/rivers

EXPOSE 8080

ENTRYPOINT ["riversd", "--config", "/etc/rivers/riversd.toml"]
