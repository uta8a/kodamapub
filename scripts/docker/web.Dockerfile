FROM node:22-bookworm-slim AS deps

WORKDIR /workspace/apps/web

COPY apps/web/package.json apps/web/package-lock.json ./

RUN npm ci

FROM deps AS build

COPY apps/web ./

RUN npm run build

FROM node:22-bookworm-slim AS runtime

ENV NODE_ENV=production
ENV PORT=5173
ENV API_ORIGIN=http://server:3000
WORKDIR /app

COPY apps/web/package.json apps/web/package-lock.json ./
RUN npm ci --omit=dev

COPY --from=build /workspace/apps/web/dist ./dist
COPY --from=build /workspace/apps/web/.output/server ./.output/server

EXPOSE 5173

CMD ["node", ".output/server/index.js"]
