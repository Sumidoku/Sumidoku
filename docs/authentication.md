# Authentication

Authentication is off until the built-in admin user has a password
(`SUMIDOKU_AUTH_PASSWORD`, or the legacy `SUMIDOKU_BASIC_PASSWORD` alias).
With a password configured, the web UI shows a login page and `/api/v1`
requires `Authorization: Bearer <token>` (a login-issued session token or
a configured API key). Unauthenticated requests get a uniform `401`
(`{"error":{"code":"unauthorized"}}`).

With auth off, every request acts as the built-in admin — a no-password
deployment behaves exactly like a single-user one.

## One owner

The built-in admin (`USER_ID = 1`) is the only identity. There are no
additional accounts, roles, user-management routes or Users page. Library,
progress, categories, sources, downloads, updates, sync, backups and all
settings belong to this owner. Authenticated requests act as admin; the API
does not expose a role field.

Login uses the admin's username and password. Existing admin sessions stay
valid across the single-owner migration. Other accounts and their sessions
are removed.

## Existing non-admin data

Migration 0019 removes all non-admin accounts and their libraries, progress,
categories, settings, downloads and sync state. There is no export, merge or
restore path for those accounts. Built-in admin data is preserved, including
legacy rows whose `user_id` is 1 and settings stored under `user.1.*`; these
legacy storage details do not represent a multi-user product model.

The release notes for this change explicitly call out the deletion as a
breaking change.

## Credentials

- **Admin password** — `SUMIDOKU_AUTH_PASSWORD` env or `auth.password` in
  `config.toml`. On every boot it is hashed (argon2) onto the built-in
  admin's user row, so the config value overrides a password previously
  changed through the UI. `SUMIDOKU_BASIC_PASSWORD` is a legacy alias
  that applies only when the main variable is unset.
- **Password changes** — the settings page requires the current password.
  Changing it revokes every admin session token.
- **Session tokens** — the login page exchanges the admin username and
  password for a bearer token (30-day rolling expiry) bound to the built-in
  admin.
- **API keys** — `SUMIDOKU_AUTH_API_KEYS` (comma-separated) or
  `auth.api_keys` in `config.toml`. Long-lived, used directly as
  `Authorization: Bearer <key>`, never stored in the database, and act
  with the built-in admin's identity.

## Notes

- `<img>` links and WebSocket clients cannot send headers; media and
  stream URLs accept `?access_token=<token>` as an alternative to the
  header.
