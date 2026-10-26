# Sumidoku

[![CI](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml/badge.svg)](https://github.com/Sumidoku/Sumidoku/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/Sumidoku/Sumidoku)](https://github.com/Sumidoku/Sumidoku/releases)
[![docker pulls](https://img.shields.io/docker/pulls/sumidoku/sumidoku)](https://hub.docker.com/r/sumidoku/sumidoku)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Self-hosted manga server: Aidoku WASM and OPDS sources, REST API, shipped as a single binary or a Docker image.

![Library](docs/images/library.png)
![Source repositories](docs/images/source-repos.png)

## Features

- **Aidoku WASM sources** — run source packages from any Aidoku-compatible
  repository: listings, search, filters, source home pages, settings,
  JavaScript execution, canvas drawing, and partial-result streaming all
  work server-side.
- **Source repositories** — add Aidoku-style source repos, browse with
  language/NSFW filters, one-click install and update.
- **Local source** — a built-in source reads manga straight from a local
  directory (`<manga>/<chapter>` image folders or CBZ files), with
  `ComicInfo.xml` metadata and cover fallback.
- **OPDS sources** — add Komga, Kavita, Calibre-Web and other OPDS 1.2 or
  2.0 catalogs as sources, with optional per-catalog credentials.
- **Library and reading** — categories and Aidoku-style filter groups
  (saved rule sets over reading state, source, content rating and
  category), reading progress, history, chapter downloads, and scheduled
  library updates.
- **Cloudflare bypass** — an optional [trawl](https://github.com/germondai/trawl)
  sidecar solves a challenged host once, then keeps routing it through trawl.
- **Real-browser login** — the same trawl sidecar doubles as the webview
  browser backend, cookies landing in the source's session.
- **Single-owner authentication** — off by default; once an admin
  password is set, username + password login guards the web UI and REST API.
  One built-in admin owns all data, with session tokens and long-lived API keys.
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

## Windows desktop

The release also ships a portable Windows app: download `sumidoku_<version>_x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/Sumidoku/Sumidoku/releases/latest), extract it anywhere and run `sumidoku-server.exe`. It listens on http://localhost:3000 only, keeps its data under `%LOCALAPPDATA%\Sumidoku`, and sits in the notification area — a left click (or Open) opens the web UI, and the menu offers the data and log folders, Start with Windows, and Exit. Started as a service or scheduled task it stays headless and takes the same configuration channels as any other install.

## Installing sources

Drop `.aix` source packages (or extracted source directories) into `./config/sources`, then press the scan button on the web UI's browse page (or `POST /api/v1/sources/scan`). You can also add an Aidoku-compatible source repository from the browse page and install sources from it directly.

## OPDS sources

Use the browse page to add Komga, Kavita, Calibre-Web or other OPDS 1.2/2.0 catalogs as sources. Multiple catalogs can be added, with optional per-catalog credentials. Sumidoku consumes these catalogs but does not expose an OPDS API.

## Documentation

- [docs/configuration.md](docs/configuration.md) — environment variables, `config.toml`, precedence
- [docs/authentication.md](docs/authentication.md) — the auth model, credentials and token details
- [docs/sidecars.md](docs/sidecars.md) — trawl (Cloudflare bypass and webview login) setup
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
