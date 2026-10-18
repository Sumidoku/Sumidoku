# Authentication

Authentication is off until `SUMIDOKU_AUTH_PASSWORD` (or the legacy
`SUMIDOKU_BASIC_PASSWORD`) is set. With a password configured, the web UI
shows a login page, `/api/v1` requires `Authorization: Bearer <token>` (a
login-issued session token or a configured API key), and the OPDS catalogs
require Basic auth with the same password. Unauthenticated requests get a
uniform `401` (`{"error":{"code":"unauthorized"}}`).

## Credentials

- **Site password** — `SUMIDOKU_AUTH_PASSWORD` env or `auth.password` in
  `config.toml`. On every boot it is hashed (argon2) into the database, so
  the config value overrides a password previously changed through the UI.
  `SUMIDOKU_BASIC_PASSWORD` is a legacy alias that applies only when the
  main variable is unset.
- **Session tokens** — the login page exchanges the password for a bearer
  token (30-day rolling expiry). Changing the password revokes all session
  tokens.
- **API keys** — `SUMIDOKU_AUTH_API_KEYS` (comma-separated) or
  `auth.api_keys` in `config.toml`. Long-lived, used directly as
  `Authorization: Bearer <key>`, never stored in the database.

## Notes

- The password can also be changed from the web UI's settings page (the
  current password is required).
- `<img>` links and WebSocket clients cannot send headers; media and stream
  URLs accept `?access_token=<token>` as an alternative to the header.
- OPDS clients authenticate with HTTP Basic: any non-empty username and the
  site password. API keys do not work for OPDS.
