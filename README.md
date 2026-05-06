# kodamapub
A minimal ActivityPub implementation for personal use.

## Development

- Backend: `cargo run -p kodamapub-server`
- Frontend: `cd apps/web && npm install && npm run dev`
- Default frontend user: `VITE_DEFAULT_USERNAME=alice`
- Docker stack: `docker compose up --build` then open `http://localhost:8080`
- Mise task: `mise run compose-up`
