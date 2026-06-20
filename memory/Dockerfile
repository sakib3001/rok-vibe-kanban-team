FROM node:22-alpine

WORKDIR /app

COPY package.json ./
COPY server.js ./
COPY migrations ./migrations

RUN npm install --omit=dev

ENV MEMORY_PORT=8091
EXPOSE 8091

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD wget -qO- "http://127.0.0.1:${MEMORY_PORT}/health" >/dev/null 2>&1 || exit 1

CMD ["node", "server.js"]
