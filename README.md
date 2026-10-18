# Sumidoku

[![CI](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml/badge.svg)](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/Sumidoku/Sumidoku)](https://github.com/Sumidoku/Sumidoku/releases)
[![docker pulls](https://img.shields.io/docker/pulls/sumidoku/sumidoku)](https://hub.docker.com/r/sumidoku/sumidoku)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Self-hosted manga server: Aidoku WASM sources, REST + OPDS API, shipped as a single binary or a Docker image.

![Library](docs/images/library.png)
![Source repositories](docs/images/source-repos.png)

## Features

- **Aidoku WASM sources** — run source packages from any Aidoku-compatible
  repository: listings, search, filters, source home pages, settings,
  JavaScript execution, canvas drawing, and partial-result streaming all
  work server-side.
- **Source repositories** — add Aidoku-style source repos, browse with
  language/NSFW filters, one-click install and update.
- **Library and reading** — categories, reading progress, history,
  chapter downloads, scheduled library updates, and OPDS 1.2 + 2.0
  catalogs for KOReader and other readers.
- **Cloudflare bypass** — an optional [trawl](https://github.com/germondai/trawl)
  sidecar solves a challenged host once, then keeps routing it through trawl.
- **Real-browser login** — an optional Chromium sidecar (driven over CDP)
  backs webview login flows, cookies landing in the source's session.
- **Authentication** — one site password guards the web UI, REST API and
  OPDS (off by default); session tokens and long-lived API keys.
- **Backup and migration** — versioned JSON backup, scheduled backups with
  retention, and imports from Aidoku (`.aib`) and Suwayomi/Mihon
  (`.tachibk`, `.proto.gz`).

## Quick start

```sh
docker compose up -d
```

The compose file pulls the published `sumidoku/sumidoku:latest` image and mounts `./config` and `./downloads`. Open http://localhost:3000. The same image is published to GitHub Container Registry as `ghcr.io/sumidoku/sumidoku`; point the compose `image:` at either.

| Volume | Purpose |
| --- | --- |
| `/config` | database, settings, cover cache, source packages |
| `/downloads` | downloaded chapters |

Everything runs with defaults; the full configuration reference (environment variables, `config.toml`, precedence) lives in [docs/configuration.md](docs/configuration.md).

## Installing sources

Drop `.aix` source packages (or extracted source directories) into `./config/sources`, then press the scan button on the web UI's browse page (or `POST /api/v1/sources/scan`). You can also add an Aidoku-compatible source repository from the browse page and install sources from it directly.

## OPDS

The OPDS 1.2 catalog is served at `/opds/v1.2` (OPDS 2.0 at `/opds/v2`). With authentication enabled, the catalogs require Basic auth with the site password; see [docs/authentication.md](docs/authentication.md).

## Documentation

- [docs/configuration.md](docs/configuration.md) — environment variables, `config.toml`, precedence
- [docs/authentication.md](docs/authentication.md) — the auth model, credentials, OPDS and token details
- [docs/sidecars.md](docs/sidecars.md) — trawl (Cloudflare bypass) and Chromium (webview login) setup
- [docs/backup.md](docs/backup.md) — backup format, scheduled backups, and the Aidoku/Suwayomi import paths
- [docs/architecture.md](docs/architecture.md) — the technical contract: API surface, WASM runtime, data flow

## Local build

The web UI is embedded into the binary at compile time, so it must be built first:

```sh
cd webui && npm ci && npm run build && cd ..
cargo build --release -p sumidoku-server
```

## Development

- `cargo nextest run --workspace` — test suite
- `cd webui && npm run dev` — Vite dev server, proxying API calls to the server on :3000

## Contributing

Issues and pull requests are welcome. The project is built and maintained by agent sessions — [AGENTS.md](AGENTS.md) is the entry point for how that works, and [docs/contributors.md](docs/contributors.md) describes how community PRs are triaged and reviewed.

## License

MIT
