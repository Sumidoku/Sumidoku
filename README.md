# Sumidoku

Self-hosted manga server: Aidoku WASM sources, REST + OPDS API, shipped as a single binary or a Docker image.

## Quick start

```sh
docker compose up -d
```

The compose file pulls the published `sumidoku/sumidoku:latest` image and mounts `./config`, `./sources` and `./downloads`. Open http://localhost:3000.

## Volumes and environment

| Volume | Purpose |
| --- | --- |
| `/config` | database, settings, cover cache |
| `/sources` | source packages, mounted in rather than baked into the image |
| `/downloads` | downloaded chapters |

| Variable | Default | Purpose |
| --- | --- | --- |
| `SUMIDOKU_ADDR` | `0.0.0.0:3000` | listen address |
| `DATABASE_URL` | `sqlite:///config/sumidoku.db?mode=rwc` | SQLite database |
| `SUMIDOKU_SOURCES_DIR` | `/sources` | sources directory |
| `SUMIDOKU_DOWNLOADS_DIR` | `/downloads` | downloads directory |
| `SUMIDOKU_COVERS_DIR` | `/config/covers` | cover cache directory |

## Installing sources

Drop `.aix` source packages (or extracted source directories) into `./sources`, then press the scan button on the web UI's browse page, or call:

```sh
curl -X POST http://localhost:3000/api/v1/sources/scan
```

## OPDS

The OPDS 1.2 catalog is served at `/opds/v1.2`. Basic auth applies to the catalog only, and only when `SUMIDOKU_BASIC_PASSWORD` is set (any non-empty username).

## Local build

The web UI is embedded into the binary at compile time, so it must be built first:

```sh
cd webui && npm ci && npm run build && cd ..
cargo build --release -p sumidoku-server
```

## Development

- `cargo test` — server test suite
- `cd webui && npm run dev` — Vite dev server, proxying API calls to the server on :3000

## License

MIT
