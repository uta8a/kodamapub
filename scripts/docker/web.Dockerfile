FROM node:22-bookworm-slim

WORKDIR /workspace/apps/web

COPY apps/web/package.json apps/web/package-lock.json ./

RUN npm ci

CMD ["npm", "run", "dev", "--", "--host", "0.0.0.0", "--port", "5173"]
