# Issue-ingestion sidecar — tiny Node service, no external deps (uses global fetch).
FROM node:22-alpine

WORKDIR /app
COPY package.json ./
COPY server.js ./

# Drop privileges; the built-in `node` user exists in this image.
USER node

ENV INGEST_PORT=8090
EXPOSE 8090

# Lightweight healthcheck against /health.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- "http://127.0.0.1:${INGEST_PORT}/health" >/dev/null 2>&1 || exit 1

CMD ["node", "server.js"]
