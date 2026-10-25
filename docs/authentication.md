# Authentication

Authentication is off until the built-in admin user has a password
(`SUMIDOKU_AUTH_PASSWORD`, or the legacy `SUMIDOKU_BASIC_PASSWORD` alias).
With a password configured, the web UI shows a login page and `/api/v1`
requires a session credential. Browser requests use the login cookie; CLI
and other non-browser clients may use `Authorization: Bearer <token>` (a
login-issued session token or configured API key). Unauthenticated requests
get a uniform `401` (`{"error":{"code":"unauthorized"}}`).

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

- **Admin username** — `SUMIDOKU_AUTH_USERNAME` env or `auth.username` in
  `config.toml`; unset or blank means `admin`. On every boot a configured
  value is written onto the built-in admin's user row. `GET
  /api/v1/auth/status` returns it as the top-level `username` field only
  with a valid authenticated identity; anonymous responses omit it, and the
  login page starts with an empty username field.
- **Admin password** — `SUMIDOKU_AUTH_PASSWORD` env or `auth.password` in
  `config.toml`. On every boot it is hashed (argon2) onto the built-in
  admin's user row, so the config value overrides a password previously
  changed through the UI. `SUMIDOKU_BASIC_PASSWORD` is a legacy alias
  that applies only when the main variable is unset.
- **Password changes** — the settings page requires the current password.
  Changing it revokes every admin session token.
- **Session tokens** — the login page exchanges the admin username and
  password for a bearer token (30-day rolling expiry) bound to the built-in
  admin. Login also sets a `sumidoku.session` cookie containing the same
  token, with `Path=/`, `HttpOnly`, `SameSite=Strict` and a 30-day maximum
  age; HTTPS responses add `Secure`. Browser API requests, including SSE,
  images and file downloads, use this cookie instead of putting credentials
  in URLs. Logout and password changes clear it; password changes also
  revoke every session token as described above.
- **API keys** — long-lived bearer credentials used directly as
  `Authorization: Bearer <key>`; they act with the built-in admin's
  identity and never create session cookies. Two sources exist:
  - **Config-side keys** — `SUMIDOKU_AUTH_API_KEYS` (comma-separated) or
    `auth.api_keys` in `config.toml`. Plaintext stays in the config, never
    in the database; the settings page lists them as read-only entries.
  - **UI-managed keys** — created in Settings → System → API keys
    (`POST /api/v1/auth/api-keys`). The plaintext key is shown once in the
    creation response; only its SHA-256 fingerprint is stored (`api_key`
    table, migration 0024) together with name, creation time and
    last-used time. Revoking (`DELETE /api/v1/auth/api-keys/{id}`) deletes
    the fingerprint, so the key dies immediately.

  Bearer session tokens remain available for CLI and other non-browser
  clients.

## Notes

- URL `access_token` parameters are not accepted. The browser uses the
  session cookie for all API requests; other clients use the bearer header.
