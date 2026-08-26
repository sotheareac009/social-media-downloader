# syntax=docker/dockerfile:1

# SocialSync — containerised build and run.
#
# WHAT THIS IS FOR. This is a Tauri desktop application: it draws a native
# window. A container has no display of its own, so this image is useful for
# two things:
#
#   1. A reproducible build. `docker compose run --rm build` produces Linux
#      binaries and bundles without installing Rust, Node or the GTK/WebKit
#      development headers on the host.
#
#   2. Running the app on a Linux host, where the X11 socket can be shared in.
#      See docker-compose.yml. On macOS and Windows this needs an X server
#      (XQuartz / VcXsrv) and is a worse experience than `npm run tauri dev`.
#
# The real payoff is that yt-dlp and FFmpeg are baked in, so the download
# engine and the 1080p+ merging work with nothing to install.

# ---------------------------------------------------------------- build stage

FROM rust:1-bookworm AS build

ARG NODE_MAJOR=22

# Tauri v2's Linux build needs the WebKitGTK and GTK development headers.
# `libxdo-dev` and the appindicator package are required by tauri-runtime-wry
# even though this app uses neither tray icons nor synthetic input.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      curl \
      file \
      pkg-config \
      libgtk-3-dev \
      libwebkit2gtk-4.1-dev \
      libsoup-3.0-dev \
      libjavascriptcoregtk-4.1-dev \
      librsvg2-dev \
      libssl-dev \
      libxdo-dev \
      libayatana-appindicator3-dev \
      patchelf \
      gnupg \
      # The .deb bundler shells out to xdg-open when generating the desktop
      # entry; without it the Rust build succeeds and then bundling fails.
      xdg-utils \
    && curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Manifests first, so a source-only change doesn't re-resolve dependencies.
COPY package.json package-lock.json ./
RUN npm ci

# Cargo refuses to parse a manifest whose declared targets are missing, and
# `[lib] name = "media_downloader_lib"` needs a src/lib.rs to exist. Stub both
# targets so dependencies can be resolved in their own cached layer; `COPY . .`
# below replaces them with the real sources.
COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./src-tauri/
RUN mkdir -p src-tauri/src \
    && : > src-tauri/src/lib.rs \
    && echo 'fn main() {}' > src-tauri/src/main.rs \
    && cargo fetch --manifest-path src-tauri/Cargo.toml \
    && rm -rf src-tauri/src

COPY . .

# `tauri build` runs the frontend build first via `beforeBuildCommand`.
RUN npm run tauri build

# -------------------------------------------------------------- runtime stage

FROM debian:bookworm-slim AS runtime

ARG TARGETARCH

# Runtime halves of the build libraries, plus the two things that make this
# image worth having: FFmpeg (without which YouTube caps at 360p) and yt-dlp.
#
# `dbus-x11` and `gnome-keyring` are here for account sign-in: the credential
# store talks to the Secret Service, which does not exist in a bare container.
# Downloading needs neither, and works without them.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      libgtk-3-0 \
      libwebkit2gtk-4.1-0 \
      libayatana-appindicator3-1 \
      librsvg2-2 \
      ffmpeg \
      dbus-x11 \
      gnome-keyring \
      curl \
    && rm -rf /var/lib/apt/lists/*

# yt-dlp ships self-contained binaries, which avoids pulling in a Python stack.
# It updates far more often than this image will be rebuilt, so it is fetched
# at build time rather than pinned to a distro package.
RUN set -eux; \
    case "${TARGETARCH:-amd64}" in \
      amd64) YTDLP_ASSET=yt-dlp_linux ;; \
      arm64) YTDLP_ASSET=yt-dlp_linux_aarch64 ;; \
      *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/yt-dlp \
      "https://github.com/yt-dlp/yt-dlp/releases/latest/download/${YTDLP_ASSET}"; \
    chmod +x /usr/local/bin/yt-dlp; \
    /usr/local/bin/yt-dlp --version

# Run as a normal user: nothing here needs root, and files written to the
# mounted downloads volume should not land on the host owned by root.
RUN useradd --create-home --shell /bin/bash app
USER app
WORKDIR /home/app

COPY --from=build /app/src-tauri/target/release/media-downloader /usr/local/bin/media-downloader

# Checks for a usable display before starting, so a container with no X server
# prints an explanation rather than a GTK panic.
COPY --chmod=0755 docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

# Where downloads land inside the container; docker-compose maps this to the
# host. The app defaults to ~/Downloads/SocialSync.
RUN mkdir -p /home/app/Downloads

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
