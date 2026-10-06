# syntax=docker/dockerfile:1

# uuid/icu crates need rustc >= 1.89; keep in sync with local toolchain when possible
FROM rust:1.89-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium \
    ca-certificates \
    fonts-liberation \
    fonts-noto-core \
    libnss3 \
    libatk-bridge2.0-0 \
    libgtk-3-0 \
    libx11-xcb1 \
    libxcomposite1 \
    libxdamage1 \
    libxrandr2 \
    libgbm1 \
    libasound2 \
    libpangocairo-1.0-0 \
    libcups2 \
    libdrm2 \
    libxshmfence1 \
    xvfb \
    x11-utils \
    procps \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/rust_scraper_timescale /app/rust_scraper_timescale
COPY static /app/static
COPY docker/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV CHROME_PATH=/usr/lib/chromium/chromium
ENV BIND_ADDR=0.0.0.0:8080
ENV USE_XVFB=1
ENV DISPLAY=:99
ENV HEADLESS=0
EXPOSE 8080

CMD ["/app/entrypoint.sh"]
