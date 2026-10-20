# Authentication

Authentication is off until the built-in admin user has a password
(`SUMIDOKU_AUTH_PASSWORD`, or the legacy `SUMIDOKU_BASIC_PASSWORD` alias).
With a password configured, the web UI shows a login page, `/api/v1`
requires `Authorization: Bearer <token>` (a login-issued session token or
a configured API key), and the OPDS catalogs require Basic auth.
Unauthenticated requests get a uniform `401`
(`{"error":{"code":"unauthorized"}}`).

With auth off, every request acts as the built-in admin — a no-password
deployment behaves exactly like a single-user one.

## Users and roles

Login is username + password. The built-in admin (username `admin`) is
seeded at first migration; further accounts are created by an admin —
there is no open registration. Two roles:

- **admin** — everything: user management, sources and repositories,
  source settings and login, server settings, backup restore.
- **reader** — own user-state only: library, progress, categories,
  downloads, and backup exports of self-data. Global writes answer `403`.

User management lives on the web UI's Users page (backed by
`GET`/`POST`/`PATCH`/`DELETE /api/v1/users`) and refuses to run while
authentication is disabled — multi-user requires setting the admin
password first. The built-in admin cannot be deleted. Deleting a user
removes their library, progress, categories, download rows and session
tokens; downloaded chapter files are shared artifacts and stay.

## Credentials

- **Admin password** — `SUMIDOKU_AUTH_PASSWORD` env or `auth.password` in
  `config.toml`. On every boot it is hashed (argon2) onto the built-in
  admin's user row, so the config value overrides a password previously
  changed through the UI. `SUMIDOKU_BASIC_PASSWORD` is a legacy alias
  that applies only when the main variable is unset.
- **User passwords** — every account has its own, set at creation or
  reset by an admin; a user changes their own from the settings page
  (the current password is required). Changing or resetting a password
  revokes all of that user's session tokens.
- **Session tokens** — the login page exchanges username + password for
  a bearer token (30-day rolling expiry) bound to that user.
- **API keys** — `SUMIDOKU_AUTH_API_KEYS` (comma-separated) or
  `auth.api_keys` in `config.toml`. Long-lived, used directly as
  `Authorization: Bearer <key>`, never stored in the database, and act
  with the built-in admin's identity.

## Notes

- `<img>` links and WebSocket clients cannot send headers; media and
  stream URLs accept `?access_token=<token>` as an alternative to the
  header.
- OPDS clients authenticate with HTTP Basic using a real username and
  that user's password — the any-non-empty-username behavior ended in
  v0.16.1. API keys do not work for OPDS. Each catalog is scoped to the
  authenticating user's own library, progress and downloads.
