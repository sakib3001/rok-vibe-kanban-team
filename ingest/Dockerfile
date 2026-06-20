# Issue-ingestion sidecar — tiny Node service, no external deps (uses global fetch).
FROM node:22-alpine

WORKDIR /app
COPY package.json ./
COPY server.js ./

# Dedup store lives here; create it node-owned so a freshly-created named volume
# inherits node ownership (Docker seeds new volumes from the image mountpoint).
# Without this the volume is root-owned and persistDedup() fails with EACCES.
RUN mkdir -p /data && chown node:node /data
VOLUME /data

# Drop privileges; the built-in `node` user exists in this image.
USER node

ENV INGEST_PORT=8090
EXPOSE 8090

# Lightweight healthcheck against /health.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- "http://127.0.0.1:${INGEST_PORT}/health" >/dev/null 2>&1 || exit 1

CMD ["node", "server.js"]
