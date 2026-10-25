# Optional sidecars

Sumidoku ships as one binary and needs nothing else for the core flow. One
optional sidecar unlocks sources that fight back:
[trawl](https://github.com/germondai/trawl), an external HTTP service
(Camoufox/Firefox inside) that solves Cloudflare challenges and doubles as
the browser backend for webview login flows. It is already wired into the
compose file with both its API (`SUMIDOKU_NET_BROWSER_TRAWL`) and forward
proxy (`SUMIDOKU_NET_TRAWL_PROXY`) enabled by default.

## Cloudflare bypass (trawl)

Sources behind Cloudflare anti-bot protection are solved through a
[trawl](https://github.com/germondai/trawl) sidecar, with its forward proxy
kept as the fallback.

`SUMIDOKU_NET_BROWSER_TRAWL` points at the trawl API (e.g.
`http://trawl:8191`, requires trawl ≥ v1.9.1); the compose file enables it by
default. A direct request answered
`403`/`503` with Cloudflare headers triggers a one-off solve through trawl's
scrape API. The answer carries the `cf_clearance` cookie and the solving
browser's user agent — CF binds the cookie to that UA (and IP), so Sumidoku
caches the pair per host and replays it on plain direct requests, which look
like the solver's Firefox (no `sec-ch-ua`, Firefox's navigation `Accept`).
A clearance the site refuses is dropped and re-solved once per request.
Pages then load at direct-request speed; a browser runs only while solving.

Without `SUMIDOKU_NET_BROWSER_TRAWL`, or when a solve fails, the fallback is
`SUMIDOKU_NET_TRAWL_PROXY`: the challenged request retries once through
trawl's forward proxy and the host is remembered, so every later request for
it also goes through trawl. Know the proxy's cost: its browser tiers answer
HTML with the rendered DOM, and a fragment response that declares no charset
is decoded by the browser's locale fallback (windows-1252) — non-ASCII text
arrives mangled. Publishing a host in
`SUMIDOKU_NET_RELAY_DOMAINS` skips the first failed direct attempt; a host in
`SUMIDOKU_NET_DIRECT_DOMAINS` is pinned to direct and opted out of solving and
escalation entirely. With neither variable set, requests go direct and the
challenge page reaches the source.

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
