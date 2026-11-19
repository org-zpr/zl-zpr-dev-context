# feature: OIDC

- Context: Part of the ZPR project. Umbrella issues: org-zpr/zl-zipline/issues/1 (OIDC), org-zpr/zl-zipline/issues/40 (silent re-authentication). Their master plans are retired; see *Design decisions*.
- Note that this started life in the org-zpr/zpr-dev-context repository
  and has been moved into org-zpr/zl-zpr-dev-context fork. So some
  references may still be pointing into the old repo.



## Vision

The zpr system has just one way for a user to authenticate. When the user
launchs the ph in adapter mode, authentication with the visa service happens
based on a pre-shared RSA public key.

I'd like to add a new way: Visa Open ID Connect. For example, using Google.

For this to work, the visa service needs to be configured with a trusted service
for this purpose (eg, Google).

To reach google:
- The google open ID API endpoint will need to be already proxied on the ZPR
  network, or
- The adapter (and maybe visa service) can speack normal IP to the google
  service (ie, not use a ZPR interface for the auth channel).

Is it possible to do this with zero configuration settings on the adapter? The
node has a way to tell an adapter about existing authentication services. But
does it have a way to tell the adapter the "type"? How will adapter know to kick
off authentication with Google?

The proposed flow:

- User starts the ZPR adapter.

- As the adapter is a CLI program, the adapter will prompt the user on the
  terminal. Something like:

```
Authentication with Google required. Press any key to launch a browser window.
```

- Browser launches to the Google OAuth login sequence.

- User enters credentials, and then browser redirects back to the local ZPR process.

- Adapter grabs the tokens (or whatever it is) and these are forwarded to the
  node as part of the ZPR authentication process.

- ZPR visa service gets the authentication message and confims authenticity of
  the tokens from Google. Not sure if this requries a call to google from the
  visa service.

- Visa service sends back success message and user is now authenticated to the
  ZPRnet.


## PLAN and DETAILS

### Summary

Add OpenID Connect as an authentication method for ZPR actors, using Google as
the first provider.

The central finding of the design work is that **OIDC is not a new
authentication architecture for ZPR.** The RFCs already model a two-sided
authentication service: an actor-facing half that issues a credential and a
visa-service-facing half that validates it and returns attributes. Neither
half was implemented. OIDC is the first real implementation of that slot, with
Google supplying the credential.

The Zode logs its own actor in, and `ph-cli` is the OIDC **Relying Party**,
using the RFC 8252 native-app pattern: a public client with no client secret,
PKCE, and a loopback redirect.
The visa service validates the resulting `id_token` offline against Google's
JWKS. Nothing about Google is proxied onto the ZPRnet, and the visa service
needs no direct internet access.

### How this differs from the Vision above

The Vision section is preserved as written. Several of its assumptions changed
during design; where they conflict, this section governs.

| Vision assumption | Resolution |
|---|---|
| "Is it possible to do this with zero configuration settings on the adapter?" | **Yes.** A public client's `client_id`, issuer URL, and scopes are not secrets, so the visa service supplies them. The Zode needs no local Google configuration. |
| "Does [the node] have a way to tell the adapter the type?" | Yes. The visa service's auth-service list carries each OIDC provider's client config (`OidcClientConfig`), and the Zode reads it directly (`auth_agent::oidc_idps`). See *Wire format changes*. |
| "The google open ID API endpoint will need to be already proxied on the ZPR network, or the adapter can speak normal IP" | Both, for different legs. `ph-cli` and the browser reach Google over normal IP. The visa service reaches only the JWKS endpoint, over the ZPRnet to a `CONNECT` proxy that tunnels to Google — so nothing of Google's is *proxied* onto the ZPRnet in the reverse-proxy sense, which would be unsafe. |
| "Adapter grabs the tokens ... forwarded to the node as part of the ZPR authentication process" | Correct, and `ph-cli` exchanges the authorization code itself. The Zode sends the visa service an `id_token` in a `reauthorize`, never an authorization code. |
| "Not sure if this requires a call to google from the visa service" | No per-authentication call. Only a periodic, cacheable, stale-tolerant JWKS fetch. |
| "the visa service needs to be configured with a trusted service for this purpose (eg, Google)" | Correct, and the declaration must scope *what Google is trusted to assert* — see `allowed_domains`. |

Additionally: the earlier hand-rolled `OAuthRsa` adapter login is
**deprecated and out of scope**. This design does not extend it. The existing
`OAuthRsa` client in `zpr-core/adapter/ph/src/auth.rs:431` and its hardcoded
TLS certificate at `auth.rs:44` are legacy and should be removed as part of
this work.

### Prerequisites and ordering

Two pieces of work should land **before** the OIDC implementation.

**1. Class specs must emit a presence condition (compiler) — [zpr-compiler#144](https://github.com/org-zpr/zpr-compiler/issues/144).** An unconstrained
class spec currently compiles to *no* client conditions — `cli_condition` is
empty and `zpr-compiler/src/fabric.rs:192` prints `(NONE)`. So:

```zpl
allow users to access services.
```

means "any actor may reach any declared service," not "any authenticated user."
The class name `users` is decorative in the compiled output. It requires no
user attributes — not even the presence of a user identity.

This is a pre-existing gap, not one OIDC introduces, but OIDC is what makes it
reachable, because OIDC is what makes people write user-centric rules. Every
class spec should compile to at least a presence condition
(`AttrOp::has`, `zpr-policy/policy.capnp:116`) on the namespace's authority
attribute:

| ZPL | Compiles to |
|---|---|
| `allow users to access services.` | `has user.zpr.authority` |
| `allow users on laptops to access services.` | `has user.zpr.authority` and `has device.zpr.authority` |

Doing this **first** fails closed: nothing authenticates a user today, so bare
user rules go from "match everything" to "match nothing," and start matching
real users once OIDC lands. Doing it second would mean shipping OIDC on
semantics we intend to change underneath it, plus a throwaway lint.

Cost: policy fixtures that use bare `users` to mean "any actor" will stop
matching and need auditing — they should say `devices` where they mean devices.
This is honest work the corpus needs anyway, since nothing in the system
currently distinguishes the two.

Prerequisite for the prerequisite: the marker attribute must be named, and
named in a space no trusted service can claim (zpr-compiler#146). Both are
specified below under *Credential lifetimes*.

**2. Identity-keyed attribute lookups (visa service) —
[zpr-visaservice#310](https://github.com/org-zpr/zpr-visaservice/issues/310).**
`connection_control.rs:452` keys all trusted-service attribute lookups on the
CN:

```rust
for ts_results in asm.ts_mgr.get_attributes_for_actor(endpoint_cn).await {
```

The `TODO(#201 follow-up)` above it notes that policy can now declare identity
attributes but this lookup runs *before* `approve_connection` assigns identity
keys, so it needs restructuring rather than a parameter change. Issue #310
tracks it and records the intended API: *send all the IDs over the link and let
the attribute store sort it out.*

**This is security-critical once user-only authentication exists.** In the
user-only case the CN is unauthenticated and attacker-chosen (see below), so an
actor could present a valid Google login for their own account while claiming a
privileged machine's CN and inherit that machine's attributes from every
trusted service. #310 is therefore a hard dependency of user-only support, not a
cleanup.

Note this was always the intent — identity attribute values were meant to be
the lookup keys. CN was simply the first and only identity ZPR had.

### Architecture

Roles, in OIDC terms:

| Role | Who |
|---|---|
| OpenID Provider | Google |
| Relying Party | `ph-cli`, for the Zode's own actor |
| End User | the human at the terminal |

Channels:

| Leg | Transport |
|---|---|
| Browser → Google (login) | normal IP, the host's existing internet path |
| `ph-cli` → Google token endpoint | normal IP |
| Google → loopback redirect | `http://127.0.0.1:<ephemeral>/callback` |
| `ph-cli` → `ph` | existing capnp RPC over the Unix socket in `get_data_home()` |
| Zode → visa service (blobs) | VSAPI `reauthorize` (`ReauthRequest.blobs`) over the Zode's VS-API session |
| **visa service → Google JWKS** | **over the ZPRnet to an on-net `CONNECT` proxy, then TLS end-to-end to Google** |

The visa service has no direct internet route. It reaches the JWKS endpoint
through an on-net HTTP forward proxy: it issues
`CONNECT www.googleapis.com:443`, then performs ordinary TLS with Google
*through* the tunnel, validating Google's certificate itself.

**The proxy is untrusted.** It sees only ciphertext, so it can deny service but
cannot forge a key set. It is a reachability mechanism, not a trust component,
and the visa-service-to-proxy leg carries ZPR's own visa enforcement and A2A
integrity on top. See *Security requirements* for the alternative model that
must not be built.

Standing up the proxy is the deployer's burden, and it is off-the-shelf
software — squid, tinyproxy, anything that speaks `CONNECT` — behind a Zode. It is declared in policy as an ordinary TCP service, so it
needs no new L7 builtin, and several trusted services may share one proxy.

**The visa service performs no OIDC discovery.** `jwks_uri` is pinned in the
ZPLC. Discovery would require a second host (Google's discovery document is on
`accounts.google.com` while the keys are on `www.googleapis.com`), doubling the
egress surface for no benefit. `ph-cli` still discovers normally, since it
has real internet access. A pinned `jwks_uri` is also more auditable — a policy
reader can see exactly what the visa service will fetch — and because policy is
signed, it is arguably a stronger assertion than a fetched discovery document.

Two properties make the fetch itself comfortable:

- The fetch is **public data, cacheable, and stale-tolerant.** If the gateway is
  down, authentication continues on cached keys.
- It is **periodic, not per-authentication**, so it adds no latency to the
  connection path — which matters, since the visa service "sits in the path of
  every new flow and is the obvious bottleneck" (`docs/VISA_SERVICE.md`).

For cold start, before any fetch has succeeded, an initial key set is seeded in
policy. Precedent: `zpr-compiler/src/policybuilder.rs:275` already embeds
trusted-service certificates via `write_service_cert`.

**Why `ph-cli` and not the visa service exchanges the code.** Having the
visa service redeem the authorization code would keep "the Visa Service
performs the actual authentication" literally true, and would let it hold the
PKCE verifier so a stolen code were useless to the endpoint. It was rejected
because it requires live outbound internet from the most security-critical
component, synchronously, on every authentication — meaning a Google outage
stops all network joins, and repeated connect attempts become an outbound-HTTPS
amplification vector. The cost of the choice is that the visa service must
implement JWT validation correctly; see *Security requirements*.

### The three authentication cases

An endpoint is a 4-tuple including an optional user identity
(`docs/SECURITY_MODEL.md`). Google authenticates a **user**; it says nothing
about the device. Both must therefore be independently expressible, and all
three combinations are supported.

| Blobs presented | `device.zpr.authority` | `user.zpr.authority` | Device CN |
|---|---|---|---|
| Device RSA only *(today)* | `zpr-bootstrap` | *absent* | **authenticated** |
| OIDC only *(new)* | *absent* | `google` | **claimed, unauthenticated** |
| Both *(new)* | `zpr-bootstrap` | `google` | **authenticated** |

The visa service implements all three on connect. Since zl-zipline#234 the
only actor `ph` authenticates is a Zode's own, which always connects with its
RSA key, so `ph` reaches only the device-only and "both" rows. On a Zode the
"both" case is reached in two steps, on one actor. The machine
authenticates as a node with its RSA key (device only). A user login then adds
the user namespace to that same actor through `reauthorize`, and a logout
removes what that login added; the device authority and the ZPR address never
change. A trusted service that vends `user.*` attributes for the device itself
re-supplies them after the logout, so a logged-out Zode is evaluated with that
device-keyed user identity, if any, not with none. That login
binds its token to a nonce the visa service issued (`getLoginNonce`), not to a
link challenge. See *Design decisions*.

Consequences:

- **`connection_control.rs:220` must accept more than one blob.** It currently
  rejects anything but exactly one, though `ConnectRequest.blobs` is already a
  list on the wire.
- **The CN must stop being unconditionally authenticated.**
  `connection_control.rs:424` pushes it into `authd_claims` on every path. With
  no device blob nothing has proven it — and `auth.rs:215` notes that adapters
  using self-generated keys send no certificate, so there is no cert CN to bind
  to either. It must go to `unauthd_claims` unless a device blob validated it.
  `authorize_connection` already takes both lists.
- **Failure rule.** A blob that is *presented and fails validation* fails the
  whole connection. A blob that is *absent* is not a failure — that namespace
  simply has no authority, and rules requiring it do not match. Silently
  downgrading a rejected user credential to device-only is rejected: it produces
  a working-but-mysteriously-limited connection and invites later holes.
- Note that bootstrap RSA authenticates a "device" only because
  `libeval/src/attribute.rs:28` namespaces the CN as `device.zpr.adapter.cn`.
  Nothing cryptographic distinguishes device from user; the credential is an
  opaque possession proof. The real distinction is **liveness** — a key file
  authenticates unattended, forever, with no human present. A deployment that
  wants an unattended *user* credential (a service account) can therefore
  declare a `[bootstrap]` entry as a user credential and receive
  `user.zpr.authority:zpr-bootstrap`. This is admissible by design but not
  implemented here.

### CLI and daemon split

`ph` is a service, eventually started by `systemd`. A daemon cannot own an
interactive browser flow. `ph-cli` already talks to `ph` over capnp RPC
(`zpr-core/adapter/admin-api/cli.capnp`), and `setCaptureFile @3` already passes
an *interface* — so capability passing is established idiom.

**`ph-cli` performs the entire OIDC flow; `ph` never talks to Google.**

```capnp
interface AuthAgent {
    # Provided by ph-cli, called by ph when a browser (and maybe a human) is needed.
    getOidcCredential @0 (issuer :Text, clientId :Text, clientSecret :Text, scopes :List(Text),
                          allowOfflineAccess :Bool, nonce :Text, interactive :Bool)
                      -> (result :SuccessOrError, idToken :Text);
}

# on CmdLineInter (zl-zipline#233):
login     @16 (authAgent: AuthAgent) -> (result: SuccessOrError);
logout    @17 () -> (result: SuccessOrError);
showLogin @18 () -> (result: Text);
```

The daemon supplies the provider the visa service lists, plus a nonce the
visa service issued (`getLoginNonce`), so the `id_token` is bound to this
login. The CLI does discovery, PKCE, browser launch, loopback listener, and
token exchange, and returns only the `id_token`.

`interactive` tells the agent whether it may involve the human.
`interactive = true` is initial login: prompt and open a browser.
`interactive = false` is renewal: satisfy the request from a stored refresh
token if `allow_offline_access` permits one, and otherwise fail rather than
surfacing a browser window the user did not ask for. See *Credential
lifetimes* for why renewal cannot be done silently through the browser.

This keeps the loopback listener and the browser in the same user session
(which is what makes the redirect work), and keeps outbound internet HTTP and
Google TLS trust decisions out of a root daemon.

**Two registrars, one interface:**

| Registrar | Shape | Use |
|---|---|---|
| `ph-cli connect` | foreground, blocking, exits when the login finishes | scripted login |
| `ph-cli auth-agent` | long-running, per-login-session, a `systemd` **user** unit | interactive login and unattended re-authentication |

This is the `polkit` / `ssh-agent` pattern: a privileged system daemon calls
back into a per-session agent for what only the session can provide.
Both call `login` with an agent attached; neither takes a link id.

**Flow inside `ph-cli`:**

1. Bind a loopback listener on `127.0.0.1:0` — literal IP, never `localhost`
   (DNS rebinding), never `0.0.0.0`. The ephemeral port goes into
   `redirect_uri`.
2. Discovery: `GET {issuer}/.well-known/openid-configuration` →
   `authorization_endpoint`, `token_endpoint`, `jwks_uri`.
3. Generate the PKCE verifier and S256 challenge, and a `state`. Use the
   daemon-supplied `nonce`.
4. Prompt, then open the browser.
5. On callback: verify `state`, serve a "you can close this window" page, shut
   the listener down immediately — single use.
6. `POST` the token endpoint with code + verifier.
7. Return the `id_token`.

**No link state.** The login rides on the Zode's VS-API session, not on a
link (see *Design decisions*). The wait for the user has its own bound,
`OIDC_USER_INTERACTION_TIMEOUT` (300 s), because other timeouts are sized for
machine-speed operations.

**`connect` blocks** until the login succeeds or fails, and returns a
meaningful exit code.

**Error taxonomy.** `LinkEvent::AuthenticationFailure` currently carries no
reason. Good feedback here is essential — these are the cases users hit, and
they demand different actions:

| Failure | Message must convey |
|---|---|
| Wrong Google account | "signed in as alice@gmail.com; this network accepts only example.com" |
| Google `access_denied` | the user declined at the consent screen |
| Timeout | no response within the interaction timeout |
| Discovery / network | cannot reach the issuer — not an auth problem |
| Visa service rejected the token | misconfiguration, **not** the user's fault; distinct exit code |
| Device blob failed | do not blame the Google login |
| ~~**Authenticated, but policy denied join**~~ | *superseded -- see Implementation status* |

The last is the sharpest: "your login failed" and "your login succeeded but
policy will not admit this endpoint" require completely different responses and
were indistinguishable. A reason code propagates visa service → Zode → CLI
(`AuthFailureReason`).

### Credential lifetimes and re-authentication

`vs/src/config.rs` hardcodes a 4-hour authentication lifetime:

```rust
pub const DEFAULT_AUTH_EXPIRATION: Duration = Duration::from_secs(4 * 60 * 60);
```

It is a compile-time constant, not even a `vs.toml` setting, and nothing
derives it from anything. It is a placeholder, and it is not set in stone.
Since zipline#118/#119 it no longer applies to a bootstrap
`device.zpr.authority`, which is stamped far-future (see *RSA bootstrap has no
source-imposed expiry* below); it survives as the fallback identity-attribute
lifetime where no authority sets one, and as the visa service's last-seen
cull age for nodes at startup. It also does not govern a user authentication: an OIDC login's
`user.zpr.authority` expiry comes from the dual clock below, driven by the
trusted service's own `expiration_seconds` and `max_auth_age_seconds`.

`docs/SECURITY_MODEL.md` already states the rule the code does not follow:
expirations are set by the source, policy may **shorten but not extend** them,
and an identity's lifetime is the minimum across its component
authentications.

**Move the authority attribute into the class namespaces.**
`libeval/src/attribute.rs:31` defines `AUTHORITY` as `zpr.authority` — outside
any class namespace, which is exactly why there is one global authority per
actor. It becomes `user.zpr.authority` and `device.zpr.authority`, each with
its own expiration, and each an identity key for its namespace.

**The `zpr.` infix is deliberate: the attribute stays reserved.** Moving into
the class namespaces buys per-namespace expiry and per-namespace identity keys.
It is not about handing the attribute to the deployment. `<class>.zpr.*` is the
established spelling for a ZPR-owned attribute living inside a class domain —
`device.zpr.adapter.cn` and the tag encoding `<class>.zpr.tag.<name>` both use
it. The bare spelling `user.authority` would instead sit in the deployment's own
attribute space, where "authority" is a plausible business attribute (signing
authority, approval authority, an org authority level), so a trusted service
could declare it in good faith and forge the marker. The cost is that the rare
method-discriminating rule reads longer; the common case never names the
attribute at all.

The governing invariant: **a `<ns>.zpr.authority` attribute is installed exactly
when a non-expired authentication exists for that namespace.** So the *presence*
of `user.zpr.authority` is the marker that a live user authentication exists
(this is what the compiler prerequisite, zpr-compiler#144, depends on), and its
*value* records the method:

```zpl
allow users to access services.                                      # any authenticated user
allow user.zpr.authority:google users to access finance-services.    # interactively authenticated
```

That invariant holds only while nothing but the visa service can install the
attribute, and today nothing enforces that: a declared trusted service can name
any key in a class domain's `zpr.` space on the right of a `returns_attributes`
mapping, the CN included. zpr-compiler#146 adds the reserved-namespace check at
that declaration site. Until it lands, the marker is advisory.

**OIDC has three clocks. Conflating them is the trap.** ZPR tracks a fourth
fact and turns the three into *two enforced bounds*.

| Value | Source | Moves on renewal? | Role |
|---|---|---|---|
| `exp` (~1h for Google) | token | yes | reject-only at validation. Nothing more. |
| `auth_time` | token | **no** | when the human actually logged in. Anchors the session ceiling. |
| `iat` | token | **yes** | when *this credential* was minted. Anchors the renewal window. |
| policy `expiration_seconds` | policy | — | the renewal cadence |
| policy `max_auth_age_seconds` | policy | — | the session ceiling |

Setting the ZPR user-authentication lifetime from the token's `exp` would make
users re-authenticate **hourly**, so `exp` is used only to reject a stale
assertion at validation time. The honored lifetime is the **minimum of two
clocks**, one fast and advancing, one slow and fixed:

```
renewal_expires = token.iat       + expiration_seconds    # advances on renewal
session_expires = token.auth_time + max_auth_age_seconds   # fixed for the session
user.zpr.authority.expires = min(renewal_expires, session_expires)
```

`max_auth_age_seconds == 0` leaves `session_expires` unbounded, which
preserves single-clock behaviour with the anchor moved from `auth_time` to
`iat`.

**The anchor had to move from `auth_time` to `iat`, and that move needs the
ceiling to be safe.** `auth_time` records when the human last typed a
password and does not move when a credential is renewed, so a refresh grant
returns an `id_token` whose `auth_time` is unchanged and the old
`auth_time + expiration_seconds` rule computes *the identical expiry* —
renewal buys nothing. Anchoring on `iat` fixes that, but on its own it means a
browser session the IdP keeps alive for weeks could be re-presented
indefinitely; the `auth_time`-anchored ceiling is what bounds it. Hence the
compiler requires `max_auth_age_seconds > 0` whenever
`allow_offline_access = true`, and `max_auth_age_seconds >= expiration_seconds`
(a ceiling shorter than the cadence makes renewal unreachable).

**The `auth_time`-to-`iat` fallback in token validation is closed for
renewable sessions.** Google omits `auth_time` unless the authorization request
carries `max_age`. With a fallback in place `session_expires` would silently
become `iat + max_auth_age_seconds` — a value that advances on every renewal,
making the ceiling unenforceable while appearing to work. So a missing
`auth_time` is a hard rejection whenever the provider declares
`allow_offline_access`, and `ph-cli` sends `max_age` on offline-capable
authorization requests purely to oblige the IdP to return the claim (OIDC Core
§3.1.2.1). Without offline access the fallback still applies, unchanged.

**Renewal re-proves the session; it does not re-authenticate the human.** The
visa service's `reauthorize` entry point cannot check the nonce: OIDC Core
§12.2 says a refresh-grant `id_token` **SHOULD NOT** carry a `nonce` claim,
and that if it does the value MUST be the original authorization request's —
absent or original, never fresh — so no comparison against a fresh challenge
can succeed. The path therefore performs no nonce check at all, accepting
both branches, and binds to the live session instead: same `sub`, strictly
increasing `iat`, unchanged `auth_time`, and a `zprAddr` that is a live actor
on the calling node. The connect path's nonce check is untouched, and so is
a Zode's own login on `reauthorize`: that is a new login, not a renewal, and
its token must carry a visa-service nonce (see *Design decisions*).
`docs/SECURITY_MODEL.md` records the delta.

**No new attribute and no new DB column.** The effective `min` is stamped onto
`user.zpr.authority` exactly as before, so `get_authentication_expiration` and
the visa-expiry clamp keep working untouched — visas still clamp to the
authentication, they just clamp to a value that now moves forward.

**RSA bootstrap has no source-imposed expiry, and since zipline#118 it has no
clock-driven expiry at all.** The source is the visa service
checking a public key from its own policy. Re-proving possession of a static key
file demonstrates nothing new: revocation in ZPR is immediate, every packet is
visa-checked and A2A-integrity-checked, and attribute freshness is handled by
*attribute* expiry, which refreshes independently. Bootstrap
`device.zpr.authority` is therefore stamped far-future (zipline#119). What
enforces a policy change instead is the install itself: every policy install
obliges every connected actor (each Zode's own actor) to
re-authenticate every authority it holds under the new policy generation
within the visa service's `reauth_deadline`, the RSA leg against the new
snapshot's bootstrap keys and the OIDC leg silently through the auth agent;
whoever cannot is revoked at the deadline (zipline#123). A failed silent
renewal does not disconnect a logged-in Zode: `zode_login::answer_policy_reauth`
falls back to device-only K2 authentication with the bootstrap key, so the
machine stays connected under the new policy generation and the user's login
is dropped. A resident `auth-agent` is then asked for a new interactive login
(`zode_login::session_replaced`, as after any session replacement), so a
browser prompt at that point is expected. Only a Zode that cannot pass K2
either is revoked. A bootstrap key
removed from policy thus takes effect within one deadline of the install,
which is a tighter and better-targeted bound than any timer.

Device and user lifetimes are **independent**. Tying the device lifetime to the
user's was considered and rejected: it couples an unattended credential to an
attended one for no security gain and forces device re-authentication churn
driven by an unrelated event. The good part — an endpoint being only as fresh as
its least-fresh component — comes from per-namespace expiry without the
coupling.

**Silent re-authentication requires a refresh token.** `prompt=none` is a
front-channel redirect requiring the browser's cookie jar; web applications use
a hidden iframe, but a native agent has no equivalent and would pop a visible
browser window on every renewal. True background renewal needs
`offline_access` and a back-channel POST, as `gcloud`, `aws sso`, and `gh` all
do. Because that is a real trade, it is a policy decision:

```toml
allow_offline_access = false   # default
```

The trade: a refresh token lets an attacker who has compromised the endpoint
keep authenticating as that user without the user present, until revoked.
Against that, it is scoped to `openid email profile` with no Google API
access — and a compromised endpoint can already harvest an `id_token`
whenever the user logs in, so what is added is persistence, not initial
access. Deployments that will not accept persistence leave it off and accept a
prompt every `expiration_seconds`.

**The token is held in memory by `ph-cli auth-agent` and nowhere else.**
`auth-agent` runs until interrupted, so the token lives in that
process, keyed by issuer, and dies with it: silent renewal for a whole working
session, no at-rest credential, no new dependency, and none of the persistence
risk weighed above. OS-keyring persistence stays deferred; it is additive (a
storage trait behind a flag) and needs no redesign. The intended limit is that
restarting the agent means an interactive login.

**Caveat — the "user's session, not the root daemon" mitigation holds,
conditionally.** `ph` needs root for the tun interface, but the control
socket is handed off after bind (`adapter/ph/src/sys/posix/socket_access.rs`,
`adapter/admin-api/src/socket_owner.rs`), so which uid holds the refresh
token depends only on how `ph` was started. `ph` started via
`sudo`/`pkexec` chowns the socket to the invoking user at
`/var/run/zpr/<uid>/control.sock` (0600); `ph` started by systemd with a
`control_group` configured (and present) leaves the shared socket in that
group (0660). In both
deployments `ph-cli auth-agent` runs unprivileged and the token lives in the
user's own session — the mitigation holds. The residual case is a `ph` with
no recorded owner and no usable `control_group` (none configured, or
configured but absent — since zipline#154 there is no implicit `zpr`
group): there the socket stays
root-only, `ph-cli` runs under `sudo`, and the refresh token sits in a
root-owned process for hours. The marginal risk there is small — root
already owns the tun device and could harvest an `id_token` at any
interactive login — and `docs/SECURITY_MODEL.md` records it as a present
conditional fact rather than inheriting the claim. The renewal itself is
unaffected in every deployment: a refresh grant is a back-channel HTTPS POST
with no browser, no desktop and no loopback listener, so root is fine for
it. Only the *interactive* leg is touched, and only in the residual case,
where the documented form is `sudo ph-cli auth-agent --no-browser` with
the printed URL pasted into the user's own browser — once per session
ceiling instead of once per `expiration_seconds`.

**When user authentication expires and renewal does not happen:** the Zode
logs the user out. At the expiry, with no renewal, ph sends a device-only
`reauthorize` (`zode_login.rs` `expire`). The user authority leaves the
actor only when the visa service accepts it: if it is refused, ph keeps the
login recorded and tries again every 10 s (`RENEWAL_RETRY_INTERVAL`) until
it succeeds, and the visa service's expiry sweep, which skips node actors,
is no fallback for this. The machine stays connected: once the visa service
accepts the device-only `reauthorize`, the OIDC login's attributes are
dropped, so new flows no longer match rules that needed them (the visa
service re-enriches the actor from its device identity, and a trusted
service may still supply `user.*` attributes for it; see VISA_SERVICE.md, "A
Zode's own login and logout"). Two limits follow. Until that `reauthorize`
succeeds, the expired user authority still caps the actor's authentication
lifetime, so the visa service can deny the Zode's new visa requests. A
stream bound under a user visa before the expiry stops at the visa's
expiry, which the login's expiry bounds: `ph` checks expiry on every packet,
by each Zode's own clock ([#274](https://github.com/org-zpr/zl-zipline/issues/274)).
`zode-oidc-test.sh` binds a user flow before the login expires and checks
that, after the expiry, it gets no reply, that a later ping asks for a
new visa, and that a ping after that gets no reply.

The visa service's authentication-expiry sweep (`vs/src/auth_sweep.rs`)
revokes expired *adapter* actors on their docking node; node actors are out
of its scope (zipline#44). Since zl-zipline#234 no actor docks on a Zode, so
the sweep finds nothing there, and a Zode acks any `revokeAuthentication` as
a no-op (`vss_worker::process_revoke_auth`). The visa service itself still
has no per-namespace revocation: `get_authentication_expiration`
(`libeval/src/actor.rs`) returns the **minimum** over the authority and
identity-key attributes, and `revokeAuthentication(addrs)` revokes an actor
wholesale.

### ZPLC configuration

A new trusted-service API, modeled on `api = "file"` — the existing precedent
for a trusted service with **no ZPR network presence**.

```toml
[trusted_services.google]
api             = "oidc"
issuer          = "https://accounts.google.com"
jwks_uri        = "https://www.googleapis.com/oauth2/v3/certs"
client_id       = "1234567890-abcdef.apps.googleusercontent.com"
scopes          = ["openid", "email", "profile"]
allowed_domains = ["example.com", "eu.example.com"]   # REQUIRED
seed_jwks       = "google-jwks.json"                  # cold-start keys
service         = "google-jwks-proxy"                 # optional; defaults to google-vs

expiration_seconds   = 43200    # honor a login for 12h from auth_time
max_auth_age_seconds = 86400    # optional: refuse an older login
allow_offline_access = false

returns_attributes = [
  "sub   -> user.oidc-subject",
  "email -> user.email",
  "hd    -> user.domain",
]
identity_attributes = ["sub"]

[services.google-jwks-proxy]
protocol = "tcp"
port     = 3128
provider = [["device.zpr.adapter.cn", "proxy1.zpr"]]

[bootstrap]
expiration_seconds = 14400   # device auth lifetime; 0 = life of the installed policy
```

**The JWKS proxy needs no hand-written ZPL.** `service` names the service ID
used in the **services** block for the JWKS `CONNECT` proxy, an ordinary
visa-service-facing on-net service, and the compiler weaves the
visa-service-to-proxy allow rule from the trusted-service declaration.

Compiler rules:

| Property | Rule | Why |
|---|---|---|
| `service` | **optional, warned if omitted** | names the JWKS `CONNECT` proxy. Omitting it means the visa service needs direct internet egress; `zplc` warns, and `--Werror` makes that fatal for deployments that care. Omission keeps the integration test simple (fake IdP on localhost, no proxy) without making a permissive posture silent. |
| `client` | **rejected** | `ph-cli` talks to Google directly; there is no on-net client-facing service |
| `provider` | **rejected** | no ZPR actor provides Google |
| `cert_path` | **rejected** | with `CONNECT`, the tunneled TLS to Google is what gets verified, against system roots; the visa-service-to-proxy hop needs no pinned certificate because ZPR already protects it |
| `jwks_uri` | **required** | the visa service does no discovery; see *Architecture* |
| `allowed_domains` | **required**; `["*"]` is the explicit "any Google account" opt-in | required-with-explicit-opt-out fails closed when forgotten |
| `identity_attributes` | **required**, must be `["sub"]` | identity attributes must be immutable and unique |
| `issuer` | `https://`, no query or fragment | discovery is runtime; the compiler must do no network I/O or builds stop being reproducible |
| `[services.google-*]` | **error if present** (other than the declared proxy) | there is no on-net service on either side |

Two deliberate calls:

**Reject `email` as an identity attribute, with a specific error message.**
Workspace email addresses are mutable and reusable after an employee leaves;
`sub` is the stable pairwise identifier. This is the highest-value new compiler
check in the feature — it catches a misconfiguration that would otherwise
silently transfer a departed employee's network access to their replacement.

**No new `zpr-*` L7 builtin.** `ZPR_L7_BUILTINS`
(`zpr-compiler/src/protocols.rs:35`) exists to generate *on-net communication
rules*. With `ph-cli` as RP there is no on-net authentication traffic to
permit, and adding an entry would recreate the hand-rolled-protocol shape being
retired.

**`allowed_domains` is a scoping of the trusted source, not an access rule.**
`docs/SECURITY_MODEL.md` puts it plainly: attributes are only as trustworthy as
their source, which is why trusted services are named in policy rather than
discovered. Trusting "Google" is trust in two billion accounts; what a
deployment means is *Google is trusted to speak for example.com*. An `id_token`
for `alice@gmail.com` is not a forgery — it is a true assertion outside the
scope of what this source is trusted for. So an out-of-domain token yields no
user identity at all, no `user.*` attributes appear, and no rule anywhere can
admit that account. This is why the domain constraint does **not** need to
appear in every policy rule.

### Compiled policy

`Service.kind.trusted @3 :Text` (`zpr-policy/policy.capnp:70`) already takes an
API name, so `trusted("oidc")` needs no union change. The IdP configuration has
nowhere to live:

```capnp
struct TrustedService {
  serviceId         @0 :Text;
  expirationSeconds @1 :UInt32;
  returnsAttrs      @2 :List(AttrMapping);
  identityAttrs     @3 :List(Text);
  oidc              @4 :OidcConfig;   # only when kind is trusted("oidc")
}

struct OidcConfig {
  issuer            @0 :Text;
  jwksUri           @1 :Text;   # pinned; the VS performs no discovery
  clientId          @2 :Text;
  scopes            @3 :List(Text);
  allowedDomains    @4 :List(Text);
  maxAuthAgeSeconds @5 :UInt32;
  allowOfflineAccess @6 :Bool;
  seedJwks          @7 :Text;   # JSON JWKS, cold start only
  jwksProxyService  @8 :Text;   # service ID of the CONNECT proxy; empty = direct egress
}
```

New capnp field numbers are backward compatible, but the visa service's minimum
compiler version must bump from 0.15.0 — the constants are
`POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH}` at `vs/src/config.rs:29-31`. This is the
canonical three-repository change: grammar and config in `zpr-compiler`, schema
in `zpr-policy`, consumer in `zl-zpr-visaservice`.

### ZPL language impact: none

No grammar change is required. Three candidates were checked:

**Policy against Google-derived attributes** — already works;
`returns_attributes` lands them in the normal namespace:

```zpl
allow domain:example.com employees to access wiki-services.
never allow domain:contractors.example.com users to access finance-services.
```

**Requiring both a user and a device authentication** — already expressible,
because `on` is positional (`docs/ZPL.md:184`):

```zpl
allow sales employees on managed laptops to access customer databases.
```

That statement *is* "an OIDC-authenticated user on an RSA-authenticated
device." With no OIDC blob the `user.*` attributes are absent and it cannot
match.

**Requiring a specific authentication method** — expressible once `authority`
is namespaced, via ordinary attribute matching (see *Credential lifetimes*).

No new ZPL statements are required of a deployment either. The visa
service-to-JWKS-proxy rule is woven by the compiler from the trusted-service
declaration — see *ZPLC configuration*.

Alternatives considered and rejected for the domain problem: forcing every rule
to use a domain-constrained subclass (opt-in security with a silent failure
mode; scatters a deployment-wide invariant across N rules), and having the
compiler silently inject the domain constraint into every user spec (introduces
action-at-a-distance, where policy text reads broader than what it compiles
to — unacceptable for a system whose goal is auditability).

### Visa service changes

| Change | Location |
|---|---|
| Accept more than one blob | `connection_control.rs:220` |
| Implement the OIDC blob arm (currently `"external auth not yet supported"`) | `connection_control.rs:253` |
| JWT validation against cached JWKS | new module |
| JWKS fetch via `CONNECT` proxy (or direct), with cache and stale tolerance | new |
| Weave the visa-service-to-JWKS-proxy allow rule from the `service` property; warn when omitted | `zpr-compiler` |
| CN to `unauthd_claims` unless a device blob validated it | `connection_control.rs:424` |
| `zpr.authority` → `user.zpr.authority` / `device.zpr.authority`, per-namespace expiry, per-namespace identity keys | `connection_control.rs:482`, `libeval/src/attribute.rs:31` |
| Teach `get_authentication_expiration` about namespaced authorities | `libeval/src/actor.rs:175` — it reads the single `key::AUTHORITY` today and would find neither new attribute |
| Instantiate a non-`file` trusted-service API | `vs/src/trusted_services/factory.rs` |
| Off-net service descriptor (no ZPR address, no port) | `vs/src/actor_mgr.rs:580` — `uri_for_service` currently errors unless the service has exactly one scope with a port, and hardcodes the `zpr-oauthrsa` scheme at `:588` |
| Identity-keyed attribute lookups | `connection_control.rs:451` — issue #310. Note the lookup runs before identity keys *and* before the ZPR address is allocated, so keys must come from `Policy::identity_attr_keys()` intersected with the authenticated claims |
| Revision cache keyed on ZPR address, purged on disconnect | `trusted_services/manager.rs:19`, `event_mgr.rs:109` (a no-op stub today) — addresses are recycled, so a stale entry would be inherited by a later actor |
| Attribute refresh is a no-op without a CN, so user-only actors never refresh | `actor_attributes.rs:116` (early return) and `:74` (paired `commit_revisions` skip) |
| Reason codes on authentication failure | VSAPI |

### Wire format changes

**Two blobs need no new framing.** `ConnectRequest.blobs` and
`ReauthRequest.blobs` are already `List(AuthBlob)`. A Zode's login sends its
device blob and its OIDC blob together in one `reauthorize`. (The design
carried them from adapter to node in a ZDP `AcquireZprAddress`; that message
is gone, see *Implementation status*.)

**New blob type:**

```json
{ "blob_type": "OIDC", "issuer": "https://accounts.google.com", "id_token": "<JWT>" }
```

plus an `oidc @2 :OidcBlob` arm on the `AuthBlob` union in `zpr-vsapi/vs.capnp`.

The `issuer` is a **selector, not a trust input** — it tells the visa service
which declared IdP to check against when several are configured. `client_id`
and `allowed_domains` always come from policy. The visa service must never take
`client_id` from the blob; that is how an attacker substitutes their own Google
application and their own user population.

**Service descriptor and ASA TLV.** `ServiceDescriptor`
(`zpr-common/src/vsapi_types/services.rs:17`) assumes an on-net service with a
ZPR address; Google has an issuer URL instead. The off-net IdP shape is
`OidcClientConfig` (issuer, `client_id`, `client_secret`, scopes,
`allow_offline_access`) on the visa service's auth-service list, which the
Zode reads over its VS-API session. No ZDP TLV is involved.

**No AAA address.** The AAA mechanism exists so a docked adapter could reach
an *on-net* auth service before it owns a ZPR address; OIDC over normal IP
never needed it, and nothing docks since zl-zipline#234.

### Security requirements

Non-negotiable, in addition to the above:

- **Match on `hd`, never on the email domain.** Google sets `hd` only for
  Workspace accounts. A *consumer* Google account can be registered against an
  arbitrary address, so it can present `email: alice@example.com` with
  `email_verified: true` and **no `hd` at all**. Any implementation checking
  `email.endsWith("@example.com")` admits an attacker-created consumer account
  impersonating the corporate domain. **Absent `hd` fails the domain check.**
- Require `email_verified` before mapping `email` to any attribute.
- Validate `iss`, `aud` (against the policy `client_id`), `exp`, `nonce`, and
  `kid`. Reject `alg: none` and any algorithm not in an explicit allowlist.
- Use a vetted JWT library. Do not hand-roll JWS parsing. Algorithm confusion
  and unverified `aud` are the classic failure modes here, and this is the code
  option A adds in exchange for keeping the visa service off the internet.
- A Zode's login `nonce` is one the visa service issued through
  `getLoginNonce`, bound to that node's actor and VS-API session, single-use
  and expiring (see *Design decisions*). An ordinary renewal carries no fresh
  nonce.
- PKCE S256 mandatory. The verifier never leaves `ph-cli`.
- Loopback redirect only, on `127.0.0.1`, single use, `state` verified.
- **Ordinary TLS verification against system roots for all Google traffic.**
  The legacy `OAuthRsa` client set `danger_accept_invalid_certs(true)` for its
  self-signed certificate; against Google that flag turns the token exchange
  into an unauthenticated channel. It is deleted, with the hardcoded
  certificate (zipline#15).
- **The JWKS proxy must be a `CONNECT` forward proxy, never a reverse proxy,
  and the JWKS URL must never be rewritten.** This is the intuitive design and
  it is catastrophic: a reverse proxy — or any scheme that substitutes the
  proxy's ZPR address for the JWKS hostname and lets TLS terminate at the
  proxy — can serve a forged key set. Whoever controls that box can then mint
  signing keys and forge **any** user's identity on the ZPRnet, elevating a
  piece of plumbing to the trust level of Google itself. With `CONNECT`, TLS is
  end-to-end and the proxy sees only ciphertext.

  The workable variant, if a `CONNECT` proxy is unavailable, is a pure TCP
  passthrough where the visa service connects to the proxy's address but still
  sends SNI for the real hostname and validates the certificate for that name
  (as `curl --resolve` does). It keeps TLS end-to-end but requires one proxy per
  upstream host, so `CONNECT` is preferred.
- Never log authorization codes, `id_token`s, refresh tokens, or PKCE
  verifiers.

### Testing

**Compiler fixtures** (`zpr-compiler/test-data/`, following
`m3-ping-and-http.zpl` and its `.zplc`): a valid `api = "oidc"` block compiles
and `zpdump` shows the `OidcConfig`; `service` / `client` / `provider` /
`cert_path` are rejected; missing `allowed_domains` is rejected; a non-`https`
issuer is rejected; **`email` as an identity attribute is rejected.**

**JWT validation — table-driven, offline, fixed vectors.** Needs a fixture
keypair and a small token minter.

| Input | Expected |
|---|---|
| valid token | accept |
| `alg: none` | reject |
| HS256 signed using the RSA public key as the HMAC secret | reject (algorithm confusion) |
| `aud` = attacker's client_id | reject |
| wrong `iss` | reject |
| `exp` in the past | reject |
| missing or mismatched `nonce` | reject |
| **`hd` absent** (consumer account) | reject |
| `hd` present, not in `allowed_domains` | reject |
| `email_verified: false` | `email` not mapped |
| unknown `kid` | reject |
| `auth_time` older than `max_auth_age_seconds` | reject |

**`zpt` evaluation tests**, extending
`zl-zpr-visaservice/integration-test/zpt-test-connect.sh`, which already asserts
`identity_keys == ["device.zpr.adapter.cn", "user.bas_id"]`: connect
device-only, user-only, and both; assert `identity_keys` and which policies
match. Once the class-presence prerequisite lands, assert that a bare
`allow users ...` rule does **not** match a device-only actor.

**Visa service unit tests.** Per-namespace authority stamping and independent
expiry; CN landing in `unauthd_claims` when no device blob was presented;
multiple blobs accepted; a presented-but-invalid blob failing the whole
connection. And, per the `AGENTS.md` rule that a found bug gets a failing test
first: **a user-only actor claiming another endpoint's CN must not inherit that
CN's trusted-service attributes.** That is the regression test for the
escalation at `connection_control.rs:452`, and it should be written before the
#310 restructure.

**`ph-cli` unit tests** (`adapter/cli/src/oidc.rs`). PKCE S256 derivation
against RFC 7636 vectors; loopback callback rejected on `state` mismatch;
listener bound to `127.0.0.1` only.

**Integration test with a fake IdP** — the only new infrastructure. A small
local OpenID Provider serving a discovery document, an authorization endpoint
that auto-redirects with no UI, a token endpoint, and a JWKS. In the netns
integration tests (`zpr-core/integration-test/`), the whole flow then runs in CI
with no Google and no browser: `--no-browser` plus an HTTP client that follows
the redirect to the loopback URL. It is also the only practical way to test
JWKS key rotation and the stale-cache path.

The fake IdP also serves refresh grants, so **the relying-party half of
renewal** is testable without Google: an authorization request carrying
`offline_access` gets a `refresh_token`, `grant_type=refresh_token` mints a
renewed `id_token` with an advancing `iat`, a fixed `auth_time` and no
`nonce` claim (§12.2's SHOULD NOT, which is also what Google does), and
`--revoke-refresh` models the user withdrawing the application's access.

That is now the **whole loop**: `zode-login-test.sh` exercises a Zode's
login, logout and silent renewal end to end, and `zode-oidc-test.sh` covers
JWKS key rotation and a withdrawn refresh grant, after which the Zode logs
the user out at the expiry.

**What CI cannot cover.** Real Google. This needs a manual release checklist: a
real Workspace domain for the happy path, a consumer gmail account to verify
the domain rejection actually rejects, and the real refresh /
`offline_access` grant — the fake IdP proves ZPR's side of the renewal, not
Google's. The `hd`-absent case in particular is the one a fake IdP is most
likely to model wrongly.

### Deferred

| Item | Why deferred |
|---|---|
| Per-namespace revocation in the visa service | `revokeAuthentication` is per actor, and install-driven re-authentication is all-or-nothing over the actor's namespaces (zipline#118). A Zode gets graceful degradation without it: at the expiry it logs the user out and keeps the device (see *Credential lifetimes*) |
| OS-keyring persistence for the refresh token | The token is kept in the agent process's memory; persistence is additive (a storage trait behind a flag) |
| VS-pushed renewal via `requestAuthentication` | **Done for policy install** (zipline#121/#123): the VS fans out `requestAuthentication` on every install and the Zode re-authenticates itself, with a renewal while a user is logged in. Zode-driven pull still covers clock-driven renewal |
| A user-held keypair bound to `sub` at first login | The cryptographically stronger alternative to skipping the nonce on `reauthorize`; revisit if that relaxation fails security review |
| Remove `ac @1` from `AuthBlob` and the `Authentication` service kind from the policy schema | Schema breaks; the producing client is already deleted (zipline#15) and the compiler no longer emits the kind, so this waits for a coordinated bump |
| `[bootstrap]` entries declared as user credentials | Admissible by design; no current need |
| Providers other than Google | The design is provider-generic; only Google is validated |
| A2A confidentiality, anti-replay, k-of-n concurrence | Pre-existing gaps, unrelated |

### Open questions

- Does a `CONNECT`-capable proxy behind a Zode already exist in the target
  environments, or does one need standing up? Providing it is the deployer's
  burden, and `zplc` will warn if the trusted service declares no `service`.
- Which HTTP client does the visa service use for the JWKS fetch, and does it
  support `CONNECT` proxying with end-to-end TLS? (`reqwest` does, and is
  already a dependency of `ph` and `ph-cli`.)
- Google's "Desktop app" client type has historically required a
  `client_secret` at the token endpoint even for public clients. Per RFC 8252 it
  is not confidential and can be distributed, but this must be confirmed
  against Google's current requirements. (Policy may carry one; see *Design
  decisions*.)
- Should `ph-cli auth-agent` ship a `systemd` user unit, or is registration
  left to the deployment?


## Design decisions

Rationale carried over from the completed master plans, which were retired once
shipped. Full plan text is in git history:
`git show a35b224:docs/plans/2026-09-02-oidc-implementation-plan.md` (umbrella
[zipline#1](https://github.com/org-zpr/zl-zipline/issues/1)) and
`git show a35b224:docs/plans/2026-09-16-silent-oidc-reauth.md` (umbrella
[zipline#40](https://github.com/org-zpr/zl-zipline/issues/40)). Decisions
already argued in the sections above are not repeated here.

**History: a docked adapter's OIDC nonce was bound to the node's link
challenge.** Before zl-zipline#234 removed the adapter dock, the adapter sent
`nonce = base64url_nopad(SHA-256(challenge))` over the node's HMAC challenge
in `ZdpInitAuthenticationPayload`, the node checked the challenge and put the
hash in `OidcBlob.nonce`, and the visa service required the token's `nonce`
to equal it. The visa service's connect path still checks `OidcBlob.nonce`.
([zipline#12](https://github.com/org-zpr/zl-zipline/issues/12),
[zipline#13](https://github.com/org-zpr/zl-zipline/issues/13))

**For a Zode's own login, the nonce comes from the visa service.** The
link-challenge binding worked because the docking node was a separate, honest
party: a stolen `id_token` carries someone else's nonce and fails. On a Zode
the endpoint is its own docking node, so it would mint its own nonce, and a
subverted Zode could replay a stolen `id_token` with its nonce until the
token expired. So the node calls `getLoginNonce`; the visa service mints a
random value bound to that node's actor and VS-API session, single-use and
expiring, and `reauthorize` accepts the login only when the token's `nonce`
equals such an outstanding nonce, which it spends as soon as the request is
classified as a login, before the device proof and the token are checked. A node's device-only self-renewal drops any user
authority it holds, including one whose provider policy no longer declares,
and a node's reauthorizations of its own actor are serialized so two
concurrent logins cannot both succeed.
With the adapter dock gone (zl-zipline#234) this is the only login path.
Accepting the replay risk for v1 and deferring the
visa-service nonce was considered and rejected by the operator.
([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**A Zode's login changes only its own actor.** `reauthorize` keeps the
exact-set rule for every adapter; only a node renewing its own actor may gain
or drop the `user` namespace, and the device RSA blob is mandatory either way.
A node cannot log a user into, or out of, any other actor. Switching
users in place is refused (the OIDC blob of a logged-in actor goes to the
renewal arm, which demands the same `sub`); log out, then in.
([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232))

**The nonce expectation is an enum, not a "skip" flag.** An ordinary renewal
on the reauth path needs no nonce check (see *Credential lifetimes*), but
connect and reauth share one validator. A Zode's own login on that path is not
a renewal: it uses `Required` against the visa-service nonce, like a
connect. It takes `NonceExpectation::{Required, SessionBound}` rather than a
boolean so the connect arm cannot be built with checking off by accident.
([zipline#43](https://github.com/org-zpr/zl-zipline/issues/43))

**Validation failures map to distinct wire error codes.** A token failing any
cryptographic or claim check (signature, `iss`, `aud`, `exp`, `nonce`, `kid`,
algorithm) is `invalidSignature`; an out-of-scope `hd` or too-old `auth_time`
is `authError`; an `issuer` with no declared trusted service is `paramError`;
no key set at all is `temporarilyUnavailable`. The split is what lets the CLI
tell "misconfiguration" from "wrong account" from "try again".
([zipline#11](https://github.com/org-zpr/zl-zipline/issues/11))

**`client_secret` is optional and carried end to end as public data.** Google's
native-app guidance marks it optional for Desktop clients but does not exempt
them, so rather than guess, policy may carry one (`""` = none) through
`OidcConfig` → `OidcClientConfig` → the Zode → `ph-cli`. Per RFC 8252 §8.5 it is not a
secret for a public client. ([zipline#2](https://github.com/org-zpr/zl-zipline/issues/2),
[zipline#16](https://github.com/org-zpr/zl-zipline/issues/16))

**The fake IdP serves real TLS rather than the compiler growing an escape
hatch.** The integration test needed a local issuer, which is not `https`.
Rather than a test-only `--allow-insecure-issuer` flag, the fake IdP serves TLS
from a test CA supplied through `SSL_CERT_FILE`, so the compiler's `https` rule
stays absolute. ([zipline#16](https://github.com/org-zpr/zl-zipline/issues/16))

**The session ceiling reuses `max_auth_age_seconds`.** It already meant "how
old may the human's login be" and was already enforced at validation, so
silent renewal needed no new `policy.capnp` field and no schema bump anywhere;
`reauthorize`, `ReauthRequest.blobs` and `Connection.authExpires` already
existed on the wire. (The node→adapter hop of the old adapter dock did not,
and needed a ZDP renewal pair, zipline#66; that pair went with the dock in
zl-zipline#234.) ([zipline#40](https://github.com/org-zpr/zl-zipline/issues/40))

**`ph-cli auth-agent` is how a human logs in; `connect` is the scripted form.**
Renewal happens only while an `AuthAgent` is registered and its process alive.
Both call the admin `login` RPC. `auth-agent` stays resident to renew;
`connect` returns when the login finishes and exists for CI and scripts,
where its exit codes (2 declined, 3 timeout, 4 IdP unreachable, 5 visa
service rejected the token, 6 policy denied, 1 anything else) are the point
(`adapter/cli/src/oidc.rs` `exit_code_for_auth_failure`). A second login
while logged in is refused. ([zipline#40](https://github.com/org-zpr/zl-zipline/issues/40),
[zipline#46](https://github.com/org-zpr/zl-zipline/issues/46),
[zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233))

**The legacy `ac @1` blob arm stays in `vs.capnp`.** The `OAuthRsa`
client that produced it is deleted, but removing a union arm is a schema break,
so it waits for a coordinated bump (see *Deferred*).
([zipline#15](https://github.com/org-zpr/zl-zipline/issues/15))

**A new login re-prompts the user; the token is not reused (wontfix).**
Every login must carry a fresh nonce from the visa service, and a
refresh-grant `id_token` SHOULD NOT carry a nonce (OIDC Core §12.2), so
neither a cached token nor a silent refresh can satisfy a new login.
Accepting one would need a login-time analogue of the renewal's session
binding, bound to a *recently replaced* session rather than a live one: a
visa-service change that widens the trust surface. That is not worth it.
Decided for the old adapter dock's link reconnect
([zipline#166](https://github.com/org-zpr/zl-zipline/issues/166)); it holds
for a Zode whose VS-API session is replaced (below).

**A Zode's own login rides on the VS-API session, not on a link.** The login
is a `reauthorize` of the node's own actor over its VS-API session. It does
not use a link, a link's actor authorization or a ZDP renewal exchange; the
one-mode change ([zl-zipline#234](https://github.com/org-zpr/zl-zipline/issues/234))
removed those with the adapter dock.
It reuses what stays: the `AuthAgent` capability and its bridge,
`AuthFailureReason`, the renewal-deadline helpers and the provider list the
node already holds. Decided with it (gate g116): a replaced session marks the
login logged out and prompts a resident agent rather than only reporting it
(q2); a policy re-authentication of a logged-in Zode is answered with a
renewal, not K2, which would log every Zode user out at every policy install
(q3); when a resident agent exits the login is kept until it expires, as
after `connect` (q5). Keeping the login across a session change would need a
visa-service change (keeping the user namespace on a node re-authentication,
or resuming with `open`); that was not taken.
([zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233))

**Login is the Zode form only; the adapter relying party is removed.** With
`ph zode` the only mode, no actor docks on a Zode, so the adapter's login
over a dock link went: the adapter relying party, the `AcquireZprAddress` /
`InitAuthentication` evidence path, node-side renewal of docked actors and
the ZDP renewal pair. The renewal lead is a fixed 300 s
(`zode_login::AUTH_RENEWAL_LEAD`, clamped to half the lifetime); the old
`[node] auth_renewal_lead_seconds` knob sized renewals of docked actors and
is refused as an unknown key, with no backwards compatibility.
([zl-zipline#234](https://github.com/org-zpr/zl-zipline/issues/234))


## Implementation status

Re-checked against the forks' `zipline` branches on 2026-09-17 (`zl-zpr-compiler`
0.18.0, `zl-zpr-visaservice` 0.19.0 with `POLICY_MIN_COMPILER_MINOR = 18`,
`zl-zpr-common` 0.27.0, `zl-zpr-core` 0.7.0).
The *A Zode's own login* entry below was checked on 2026-11-14 against
`feature/zodes` (`zl-zpr-visaservice` `v0.22.0-zodes.6`, `zl-zpr-common`
`zl-v0.32.0-zodes.4`) plus `zl-zpr-core` #120. The zl-zipline#234 entries
were checked on 2026-11-18 against `zl-zpr-core` `feature/zodes` (2a9d380)
plus the last #234 core PR.
**The OIDC design in this document is implemented**: the umbrella epic
[zipline#1](https://github.com/org-zpr/zl-zipline/issues/1) is closed, and the
oidc + file interplay epic
[zipline#22](https://github.com/org-zpr/zl-zipline/issues/22) has merged its
code half, including the end-to-end fixture. Where the design sections above
and the code disagree, the code wins; the decisions that changed the design
during the build are under *Design decisions*.

**Adapter docking and the adapter relying party are removed**
([zl-zipline#234](https://github.com/org-zpr/zl-zipline/issues/234)). The
design sections above were written for an adapter logging in over its dock
link. Now `ph zode` is the only mode and login is the Zode form only
(zl-zipline#233, below): the Zode authenticates its own actor to the visa
service, and `ph-cli` is the relying party for it. Gone with the dock: the
AcquireZprAddress / InitAuthentication evidence path, adapter auto-connect
and idle mode, renewal of docked actors, and ZDP types 138-145 (including
the zipline#66 renewal pair `RenewAuthenticationRequest`/`Response`, 144/145),
which are reserved and refused as unknown types (`adapter/ph/src/zdp.rs`).
`ph-cli connect` and `auth-agent` take no link id, and exit code 7 is gone.
`[node] auth_renewal_lead_seconds` is refused as an unknown key. What stays:
`auth_agent.rs` (the AuthAgent bridge and `AuthFailureReason`),
`actor_renewal.rs` (renewal timing) and `zode_login.rs`.

**Silent re-authentication** was built separately, umbrella
[zipline#40](https://github.com/org-zpr/zl-zipline/issues/40) (closed), and is
what the *Credential lifetimes and re-authentication* section above describes.

**Implemented (the prerequisites):**

- **Class specs emit a presence condition** — zpr-compiler#144, merged as
  PR #145. `allow users to access services.` compiles to
  `has user.zpr.authority`; the marker keys are `user.zpr.authority` and
  `device.zpr.authority` (`zpr-compiler/src/zpl.rs`). Compiler version 0.16.0.
- **Identity-keyed trusted-service lookups** — zpr-visaservice#310, merged as
  PR #320. Lookups use `Policy::lookup_identity_keys()`; the revision cache is
  keyed by ZPR address and purged on disconnect; attribute refresh works for
  actors with no CN.
- **CN is not promoted to an authenticated claim** by `authorize_connection`;
  only the RSA path does so after verifying the signature.
- **The `zpr.` sub-namespace is reserved from declared trusted services** —
  zpr-compiler#146, merged as PR #147 (`zl-zpr-compiler` e2eecd6). The authority
  marker is now enforced, not advisory.
- **An `api = "oidc"` service is retained regardless of whether policy
  references its attributes** — zipline#23 (`zl-zpr-compiler` PR #4,
  `Weaver::retain_identity_vendors`). The weaver's reference-based pruning
  keeps a trusted service only when ZPL names one of its returned attributes
  (or another retained service's provider depends on it — see `docs/ZPL.md`);
  an OIDC provider declares `identity_attributes`, and those are the lookup
  keys of every attribute store in the policy, so it is now woven
  unconditionally. Before this, a configuration whose only policy statement
  referenced a `file` store's attribute compiled with the OIDC provider pruned
  and every login failed with no OIDC service in the fabric.
- **Namespaced authority attributes in the visa service** — zpr-visaservice#324,
  merged as PR #331 (`zl-zpr-visaservice` 72230cf). `device.zpr.authority` and
  `user.zpr.authority` replace the single `zpr.authority`,
  `POLICY_MIN_COMPILER_MINOR` is 16, and `get_authentication_expiration` takes the
  minimum over both authorities and the identity keys. A compiler-0.16 policy's
  bare `allow users ...` rule now behaves as designed.

**Implemented (the design itself):**

- **Schema mirrors** — `OidcConfig`, `OidcBlob`, `ServiceT`, `OidcClientConfig`
  (zipline#4, `zl-zpr-common` 0.26.0). The forks' cross-repository dependencies
  were repointed at `mkolehmainen` first (zipline#17, plan issue A0, closed), so
  schema changes reach their consumers.
- **`api = "oidc"` in the compiler** — the declaration is parsed and validated
  (zipline#5) and compiled to `OidcConfig`, with the JWKS-proxy access rule
  woven (zipline#6). `zl-zpr-compiler` 0.17.0.
- **Multiple auth blobs** — the visa service accepts one blob per identity
  namespace, and a presented-but-invalid blob fails the whole connect
  (zipline#7). The old one-blob rejection (plan issue C1) is gone.
- **JWT validation** — offline `id_token` validation with vector tests
  (zipline#8).
- **The JWKS key source** — seeded keys, `CONNECT`-proxied refresh, stale
  tolerance (zipline#9), plus the ActorDb-backed proxy resolver and periodic
  refresher (zipline#19).
- **The `oidc` trusted-service store and IdP service descriptor** (zipline#10),
  with per-issuer identity scoping via `user.zpr.authority`
  (`vs/src/oidc/store.rs`).
- **The connect path** — OIDC blobs validated on connect,
  `user.zpr.authority` stamped from the vouching service,
  `POLICY_MIN_COMPILER_MINOR` raised to 17 (zipline#11); `zl-zpr-visaservice`
  0.19.0.
- **The CLI flow** — the `AuthAgent` capability and the `ph-cli`
  relying-party flow (`adapter/cli/src/oidc.rs`), and `--no-browser` for
  headless runs (zipline#20). Adapter idle mode (zipline#28) went with the
  adapter (zl-zipline#234).
- **Authority ownership across refresh** — `derive_user_authority` keeps the
  authenticating service's authority; a decorating attribute store never
  displaces it (zipline#24–#26).
- **Address-keyed admin surface** — the admin actor API and dashboard are keyed
  on ZPR address, so an OIDC-only actor with no CN is visible (zipline#31,
  zipline#32).
- **End-to-end fixtures** — the fake-IdP harness (zipline#16; now used by
  `zode-login-test.sh` and `zode-oidc-test.sh`) and the
  oidc + file trusted-service interplay test (zipline#27, `zl-zpr-core`
  02b730d): a `zpdump` pruning-regression pre-check, connect and visa
  assertions, and refresh legs covering the authority-collision regression
  recorded under `docs/VISA_SERVICE.md`'s *Design decisions*.

**A Zode's own login (Zodes epic):** built end to end. The visa service
implements a node's own user login and logout through `reauthorize`, with the
`getLoginNonce` nonce ([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232)).
ph and `ph-cli` do the node side ([zl-zipline#233](https://github.com/org-zpr/zl-zipline/issues/233),
`adapter/ph/src/zode_login.rs`):

- **CLI.** `ph-cli connect` and `ph-cli auth-agent` take no link id: they
  call the admin RPC `login` with the interactive `CliAuthAgent`. `connect`
  exits with the usual exit codes; `auth-agent` stays resident.
  `ph-cli logout` and `ph-cli login-status` are new. A second login while
  logged in is refused. There is no other login.
- **Login.** ph picks the first advertised OIDC provider, calls
  `getLoginNonce`, asks the agent interactively for a token bound to that
  nonce, and sends `reauthorize` with the device leg plus the OIDC blob. The
  device leg is signed inside libnode2's `VSConn` with the key it already
  holds. The wait for the user is bounded by
  `min(OIDC_USER_INTERACTION_TIMEOUT, nonce expiry)`. A
  `temporarilyUnavailable` answer is retried once with a new nonce.
- **Renewal.** A timer renews silently ahead of expiry
  (`renewal_deadline` with a fixed 300 s lead, `zode_login::AUTH_RENEWAL_LEAD`,
  clamped to half the lifetime; there is no setting) with a non-interactive
  agent request and the login's nonce, which the renewal arm ignores.
  Overlapping renewals share one and are spaced a second apart, since
  same-second refresh tokens can share an `iat`. A failed renewal is retried
  until the expiry; **at the expiry ph logs the user out itself** (`[SS]`
  only). The user authority goes only when the visa service accepts that
  device-only `reauthorize`; a refused one is retried every 10 s, and until
  then the user authority stays on the actor.
- **Logout** sends the device leg alone. The machine stays connected.
- **Session replaced.** After every successful node authentication (a VS-API
  reconnect, a visa-service restart, a rejoin, a K2 re-authentication) ph
  marks the login `SessionReplaced`, because the visa service built a new,
  device-only actor; a resident `auth-agent` is asked for a new interactive
  login with a new nonce. A login still in flight when the session is
  replaced fails with `SessionReplaced`, and ph logs out to be sure, since
  the replacement may have come before or after its `reauthorize`.
- **Policy re-authentication.** A `requestAuthentication` for the node's own
  address while logged in is answered with a renewal, which stamps the new
  `zpr.vinst` and keeps the user; K2 runs only when no renewal can be had.

Covered by unit tests against a fake visa service (`fake_vs.rs`), and end to
end by the netns script `zode-login-test.sh` (core CI runs it as its own
job): logged out, then `connect`, `logout`, a policy install answered by a
renewal, silent renewal past the login's lifetime, and a visa-service
restart that drops the login and prompts the resident agent again.

**Implemented (silent re-authentication, umbrella zipline#40):**

- **`allow_offline_access` validation** — the compiler requires
  `max_auth_age_seconds > 0` with offline access on, and
  `max_auth_age_seconds >= expiration_seconds` (zipline#41). `zl-zpr-compiler`
  0.18.0.
- **The dual clock** — `ValidatedToken` carries `iat`, the OIDC store records
  `auth_time` and `iat` beside the attrs, `user.zpr.authority` expires at
  `min(iat + expiration_seconds, auth_time + max_auth_age_seconds)`, and the
  `auth_time`-to-`iat` fallback is a hard rejection when the provider allows
  offline access (zipline#42). `POLICY_MIN_COMPILER_MINOR` raised to 18.
- **`reauthorize` with session-bound validation** — `vs.capnp`'s
  `reauthorize` is implemented against a pinned `PolicySnapshot` with
  `authorize_connection` re-run, so a policy change since connect applies.
  For an ordinary renewal the nonce is ignored (on this path only); same
  `sub`, strictly increasing `iat`, unchanged `auth_time` and a live actor on
  the calling node bind it instead (zipline#43). A Zode's own login on
  `reauthorize` is not a renewal: its token must carry an unexpired, unused
  visa-service nonce from `getLoginNonce`
  ([zl-zipline#232](https://github.com/org-zpr/zl-zipline/issues/232)).
- **Authentication-expiry sweep and real `revokeAuthentication`** —
  `vs/src/auth_sweep.rs`, one batched revoke per docking node, removal only on
  a positive ack, and a concurrent renewal wins (zipline#44). It covers
  adapter actors only; since zl-zipline#234 none dock, and a Zode acks a
  `revokeAuthentication` as a no-op (`vss_worker::process_revoke_auth`).
- **Renewal timing** — `actor_renewal.rs`: the renewal deadline at
  `auth_expires - min(lead, lifetime/2)` and the bound on one attempt
  (zipline#45). It once drove per-link renewal of docked actors with
  `[node] auth_renewal_lead_seconds`; it now serves only the Zode's login,
  with the fixed 300 s lead.
- **`ph-cli auth-agent` refresh grants** — `offline_access` and `max_age` on
  the authorization request when policy allows it, the `refresh_token` kept in
  the agent process's memory keyed by issuer, `interactive = false` satisfied
  by a `grant_type=refresh_token` POST, `invalid_grant` dropping the stored
  token, and the headless/root browser fallback (zipline#46). The refresh token
  is never logged, never written to disk, and never returned over the RPC.
- **Fake-IdP refresh support** — the harness serves `offline_access` and
  `grant_type=refresh_token` with an advancing `iat`, a fixed `auth_time` and
  no `nonce` claim (OIDC Core §12.2's SHOULD NOT, which is what Google does),
  plus `--revoke-refresh` (zipline#47).
- **`authorize_connect` returns the authentication expiry as `authExpires`** —
  zipline#86 (`zl-zpr-visaservice` PR #31). Root cause of the renewal never
  firing even with every hop in place: `authorize_connect` answered with the
  **ZPR-address attribute's** expiry — the ~100-year `NEVER_EXPIRES` default —
  so the node computed a renewal deadline a century out and never scheduled
  silent renewal; `reauthorize` separately computed a `USER_AUTHORITY`-only
  expiry that came out `UNIX_EPOCH` for device-only actors. Both
  `Connection::new` sites now share `connect_auth_expires(actor)`:
  `get_authentication_expiration()` (the minimum over both authorities and the
  identity keys) with the address expiry only as the no-authentication
  fallback.

**Superseded:**

- **A validated login that matches no join policy is *not* refused.** The spec's
  `policyDenied` row above and the plan's Contract 2 had the visa service reject
  such a connect; that is reverted (`connection_control.rs`, `libeval`
  `approve_connection`). A join policy grants a role and services, so an endpoint
  without one can reach nothing regardless -- the same #227 fallthrough the
  device-only path always had. "Admit only logins vouched by trusted service X" is
  a real want, but it is a policy feature ZPL cannot express yet, and refusing the
  join was the wrong lever for it. `ErrorCode::policyDenied` stays in the wire
  schema, unused by this path.

**Not yet:**

- CI runs the netns end-to-end legs against the visa service *release*, not
  against a visa service branch. The OIDC legs are `zode-login-test.sh`,
  `zode-oidc-test.sh`, `oidc-file-interplay-test.sh`,
  `fake-idp-smoke-test.sh` and `attr-query-test.sh`. CI does not have
  `zpdump`, so the interplay and attribute-query tests skip their `zpdump`
  pre-check there. A visa service change still needs the local netns tier
  ("Gate escalation" in `skills/zpr/SKILL.md`).
- Real-Google validation is a manual release checklist
  (`zl-zpr-core/integration-test/OIDC-RELEASE-CHECKLIST.md`), not automated —
  the fake IdP cannot prove the `hd`-absent rejection against Google's actual
  behavior.
- The `OAuthRsa` client in `ph`, its hardcoded certificate and
  `danger_accept_invalid_certs` are deleted (zipline#15), and the compiler no
  longer has the `zpr-oauthrsa` L7 builtin or emits `Authentication` services.
  The `ac @1` arm in `vs.capnp` and the visa service's handling of the
  `Authentication` service kind remain (see *Deferred*).

Line numbers cited in the design sections above are from the 2026-09-01
checkouts and most have moved; verify against the source before relying on
one.
