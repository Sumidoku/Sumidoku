# Optional sidecars

Sumidoku ships as one binary and needs nothing else for the core flow. One
optional sidecar unlocks sources that fight back:
[trawl](https://github.com/germondai/trawl), an external HTTP service
(Camoufox/Firefox inside) that solves Cloudflare challenges and doubles as
the browser backend for webview login flows. It is already wired into the
compose file.

## Cloudflare bypass (trawl)

Sources behind Cloudflare anti-bot protection are fetched through a
[trawl](https://github.com/germondai/trawl) sidecar: a challenge-aware
forward proxy that passes ordinary traffic through and solves detected
challenges with a real browser.

With `SUMIDOKU_NET_TRAWL_PROXY` set, a direct request answered `403`/`503`
with Cloudflare headers is retried once through trawl, and that host is
remembered: every later request for it also goes through trawl. A
`cf_clearance` cookie is bound to the user agent (and IP) that solved the
challenge, so replaying it from Sumidoku's own client with a different user
agent is refused; routing the host through trawl instead lets trawl keep the
clearance and the solving browser's user agent together (its plain-HTTP tier
replays both, so no browser runs per request). Publishing a host in
`SUMIDOKU_NET_RELAY_DOMAINS` skips the first failed direct attempt; a host in
`SUMIDOKU_NET_DIRECT_DOMAINS` is pinned to direct and opted out of the
escalation entirely. Without `SUMIDOKU_NET_TRAWL_PROXY`, behavior is
unchanged: requests go direct and the challenge page reaches the source.

HTTPS sources also need `SUMIDOKU_NET_TRAWL_CA`: trawl terminates TLS
locally to inspect responses and re-signs with its own root CA, which the
client must trust. The compose file already runs the sidecar and shares the
CA through the `trawl-ca` volume — trawl writes `ca.crt` into it on first
boot, Sumidoku reads it from `/trawl-ca/ca.crt`.

## Browser-backed webview (source login)

Sources that log in through a web view (load the login page in a real
browser, then call their API with the session cookie) use the same trawl
deployment as their browser backend: point `SUMIDOKU_NET_BROWSER_TRAWL` at
the trawl API (e.g. `http://trawl:8191`, requires trawl ≥ v1.9.1) and each
webview becomes a named keepPage session — cookies never bleed across
webviews. Every page load flows the
session's cookies into the source's session jar, so once the login page
completes, the source's own API requests carry the session. Without the
variable, behavior is unchanged: the navigation/cookie webview functions
report themselves unsupported in the source log.

Trawl-backed webviews run `webview_eval` in the real page, but
`webview_delete_cookie` is unsupported (trawl has no cookie-edit endpoint).
Two upstream behaviors to know: trawl sessions live in the process and die
with the container (Sumidoku recreates them on demand), and trawl rejects
pages under 100 characters as empty shells — real source pages never come
close, but a hand-built fixture will trip it.

### Migrating from `browser_ws`

Earlier releases drove a user-supplied Chromium over CDP, configured with
`SUMIDOKU_NET_BROWSER_WS` / `browser_ws`. That backend is gone: a leftover
`browser_ws` key is ignored (no error), and the interactive "open the login
page in a browser" flow it powered is removed — the trawl session API has
no screencast or input surface. To keep browser-backed webviews, set
`SUMIDOKU_NET_BROWSER_TRAWL` to the trawl API as above; the compose file no
longer carries the `chromium` service. Source logins that need captcha,
2FA or other interactive steps use the paste-cookie path, which is
unchanged.
