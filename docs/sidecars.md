# Optional sidecars

Sumidoku ships as one binary and needs nothing else for the core flow. Two
optional sidecars unlock sources that fight back: trawl for Cloudflare
challenges, and a Chromium instance for webview login flows. Both are
already wired into the compose file (trawl on by default, Chromium behind a
profile).

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
browser, then call their API with the session cookie) need a Chromium
sidecar: Sumidoku ships no browser, it drives one over CDP. Point
`SUMIDOKU_NET_BROWSER_WS` at the sidecar's CDP websocket — for the
[browserless](https://ghcr.io/browserless/chromium) image that is
`ws://<host>:3000/chromium`; other CDP-capable images (e.g. headless-shell
on :9222) work the same way, with whatever path that image serves.

The compose file carries an optional `chromium` service, off by default:

```sh
docker compose --profile browser up -d
```

and `SUMIDOKU_NET_BROWSER_WS=ws://chromium:3000/chromium` (a commented line
is already in the compose environment). Each webview a source opens gets an
isolated browser context — cookies never bleed across webviews — and after
every page load the context's cookies flow into the source's session jar,
so once the login page completes, the source's own API requests carry the
session. Without the variable, behavior is unchanged: the
navigation/cookie webview functions report themselves unsupported in the
source log.

A trawl deployment can double as the webview browser instead of a Chromium
sidecar: point `SUMIDOKU_NET_BROWSER_TRAWL` at the trawl API (e.g.
`http://trawl:8191`) and each webview becomes a named keepPage session with
the same cookie reflux. The two variables are mutually exclusive —
configuring both fails startup. Trawl-backed webviews additionally run
`webview_eval` in the real page, but `webview_delete_cookie` is unsupported
there (no endpoint), and trawl cannot power the interactive login below
(the session API has no screencast or input surface).

## Interactive web login (no devtools)

With the same variable set, a `Login` setting of `method: "web"` gains a
second path next to pasting cookies: "在浏览器中打开登录页". The server
opens an isolated browser context on the sidecar, loads the login page,
and streams it into the source settings modal as a live view (CDP
screencast out, mouse/keyboard back in, over one authenticated WebSocket
to the Sumidoku server). The user completes sign-in there — captcha, 2FA,
whatever the site needs — and clicks 完成登录; the server reads the
context's cookies for the login host and runs the same pipeline as pasted
cookies. The sidecar needs no UI of its own: the browserless OSS
`/debugger/` is a code workbench whose session links open plain DevTools,
and its interactive LiveURL is a premium feature, so Sumidoku relays the
session itself and any CDP-capable Chromium works.

Deployment notes:

- **Session timeouts.** A login session lives up to 10 minutes server-side;
  the sidecar must not kill the connection earlier. browserless defaults
  to `CONNECTION_TIMEOUT=30s`, which destroys the session mid-login — the
  compose file sets `CONNECTION_TIMEOUT=600000` for this reason.
- **Exposure.** The interactive view is served by Sumidoku behind its own
  auth; the sidecar stays on the private network and needs no published
  port (the compose service exposes none). Whoever can reach the Sumidoku
  UI can drive a login browser — treat the UI's own access control as the
  boundary, as with every other settings operation.
- **Scope.** Cookies are harvested for the login host (including its sub-
  and parent domains). Flows whose session cookie lands on an unrelated
  domain (some cross-domain SSO) are out of scope — the paste path remains
  for those.
