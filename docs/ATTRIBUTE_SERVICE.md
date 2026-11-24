# Attribute Services and the `zpr-attr/1` API

How the visa service obtains actor attributes from a networked source, and the
wire protocol such a source implements. The first implementer is **zipline**,
the hosted-ZPRnet product, whose database of per-actor attributes the visa
service must query. The protocol is deliberately small enough that an
organisation can put it in front of any attribute source — an LDAP directory,
Active Directory, Okta, a spreadsheet — and have the visa service consume it
natively. Think of it as the reference adapter layer for attribute sources.

Read this before changing an attribute store in the visa service, the
`api = "zpr-attr/1"` trusted-service declaration, or anything that implements
the protocol. Read it alongside [VISA_SERVICE.md](VISA_SERVICE.md) (how
attributes are refreshed and when a visa is denied) and
[SECURITY_MODEL.md](SECURITY_MODEL.md) (what an attribute's provenance means).

**Status.** Implemented as of 2026-09-22, by zipline issues #74–#81 under
umbrella [zipline#72](https://github.com/org-zpr/zl-zipline/issues/72).
This document and the code are current; the rationale behind the design is
under *Design decisions*. The wire protocol is also rendered as
`docs/zpr-attr-v1.openapi.yaml`. The `## Implementation status` section at the
end records version floors and what was deliberately not built.

---

## Sources

| Source | Status |
|---|---|
| `zl-zpr-visaservice/vs/src/trusted_services/` | The store trait and the two stores that exist today (`file`, `oidc`). |
| internal RFC-13.1, *Authentication, Identity and Attributes* | Design intent for trusted services generally. This document departs from it in one place, recorded under *Security*. |
| [RFC 7643](https://www.rfc-editor.org/rfc/rfc7643) §7, SCIM 2.0 schema definitions | The attribute-definition vocabulary `GET {url}/schema` returns. |
| `docs/zpr-attr-v1.openapi.yaml` | Machine-readable rendering of the wire protocol. This document is normative; the OpenAPI file follows it, and V3's contract tests hold the reference server to both. |

---

## The model

A **trusted service** is anything policy names as a source of attributes.
Today three kinds exist:

| Kind | Declared as | Authenticates? | Vends attributes? |
|---|---|---|---|
| bootstrap | `[bootstrap]` | yes — RSA key challenge | the device CN only |
| OIDC provider | `api = "oidc"` | yes — validated `id_token` | the token's claims |
| file store | `api = "file"` | no | a local `<id>.json` |

An **attribute service** — `api = "zpr-attr/1"` — is a fourth kind and belongs
in the last row's column: it **never authenticates anyone**. It answers one
question, *what do you know about this actor?*, for an actor named by the
identities other services have already established. It therefore behaves
exactly like a `file` store from the visa service's point of view — a
*decorating* source, keyed on identity attributes it did not itself vend — and
inherits every rule that already governs one:

- **Queried by identity attributes only.** The lookup set is computed from the
  authenticated claims before any store is consulted
  (`vs/src/trusted_services/mod.rs`, `lookup_identities`), so an attribute
  service can only be keyed on a declared identity attribute (`user.sub`, the
  device CN) plus the reserved authority marker. See VISA_SERVICE.md,
  *Authenticating actors*, for why that is structural.
- **It cannot own `user.zpr.authority`.** `derive_user_authority` refuses to
  mint an authority over one held by a different source; a decorating store
  never displaces the credential verifier (SECURITY_MODEL.md, *The authority
  marker belongs to the credential verifier*).
- **It fails closed.** A store error makes the actor's claim set
  *indeterminate* and the visa is denied; an unknown actor is an empty answer,
  not an error; two matched identities that disagree on a value are an error,
  never a coin flip.

Every attribute is a tuple `(key, values, expiry)`. The visa service adds two
things the service does not send: the **source** (the trusted-service id, so
the refresh path can prune what a source stops vending) and the **type** —
single-valued, multi-valued or tag — which comes from policy's
`returns_attributes` mapping, not from the service. A service returns names
and values; policy decides what they mean.

Mostly the visa service just hands attributes to the policy evaluator, but a
few **well-known attributes** it acts on itself. **`device.zpr_addr`** is one
(zipline#99): an authenticated `device.zpr_addr` vended by any trusted
service — an attribute service, a `file` store keyed on the device CN —
assigns the device that ZPR address at connection time, independently of
anything the peer requests: a device that asks for no address still comes up
at the granted one, and a peer request cannot select or override the value.
The grant is subject to the same checks as a node's pinned static address; a
malformed or conflicting grant rejects the connection loudly. See
VISA_SERVICE.md, *Authenticating actors*, for the rules. A store vending only
`device.zpr_addr` needs no ZPL reference to stay woven: like `device.hostname`,
it is a visa-service-interpreted attribute, and the compiler retains any
trusted service vending one
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105)), emitting
an `info` diagnostic per retention.

### The four operations

There are four things a visa service might want from an attribute source.
`zpr-attr/1` specifies the first two, provides the third through the visa
service's existing invalidation machinery, and reserves the fourth.

| Operation | What it is | In `zpr-attr/1` |
|---|---|---|
| **QUERY** | "Give me everything you have for this actor", optionally narrowed to the keys policy cares about. | `POST {url}/query` |
| **SCHEMA** | The attribute names and types the service can return. Useful to policy editors; the visa service uses it to catch a misspelt mapping at install time. | `GET {url}/schema` |
| **STREAM** | "Tell me when this actor's attributes change." | Inverted: the service notifies the visa service over the admin API, which already knows how to re-query and revoke. See *Change notification*. |
| **SATISFIES** | "Which actors satisfy `role=consultant, location=finland`?" — needed for policy assertions. | Reserved as `POST {url}/satisfies`. Not specified. |

---

## Declaring an attribute service in policy

In the `.zplc` configuration:

```toml
[trusted_services.zipline]
api = "zpr-attr/1"
url = "https://attrs.zipline.example/tenant-7"   # base of the API; https required
ca_cert_path = "zipline-ca.pem"                  # optional: PEM, embedded in the policy
timeout_seconds = 5                               # optional: 1..=30, default 5
expiration_seconds = 3600                         # required: how long returned attributes live
returns_attributes = [
  "dept -> user.dept",             # single-valued
  "roles -> user.role{}",          # multi-valued
  "contractor -> #user.contractor" # tag
]
```

| Property | Rule |
|---|---|
| `url` | Required. `https` with a host, no query or fragment. May carry a path prefix; the visa service appends `/query` and `/schema`. A trailing slash is normalised away. |
| `ca_cert_path` | Optional. Path, relative to the `.zplc`, of a PEM file holding one or more `CERTIFICATE` blocks. Its **contents** are embedded in the compiled policy, so the pin is signed along with everything else. When present it is **exclusive**: the visa service trusts only these roots for this service and disables the built-in system roots, so a certificate for the same hostname chaining to a public CA is rejected. Absent means system roots. |
| `timeout_seconds` | Optional. Whole-request timeout the visa service applies to every call. Default 5, maximum 30. |
| `expiration_seconds` | Required and positive. Default lifetime of every attribute returned, and the ceiling on any lifetime the service asks for. The visa service enforces the same 60-second floor as the file store. |
| `returns_attributes` | Required, at least one mapping. Service-side names on the left, ZPR names on the right, the usual `{}` and `#` markers. |

Rejected, each with its own diagnostic: `identity_attributes` (a decorating
store is keyed on other services' identities, the same rule as `api = "file"`),
`provider`, `client`, `cert_path` and `prefix` (network-service properties
with no meaning here). **`service` is reserved**: it will one day name
a ZPR service through which the visa service reaches an on-net attribute
service, on the `jwks_proxy_service` pattern; in `zpr-attr/1` it is rejected
with a message saying so. Reaching the service over ordinary IP is the only
mode.

The weaver treats an attribute service exactly like a `file` store: retained
when a policy statement references one of its attributes, or when it vends a
visa-service-interpreted attribute (`device.zpr_addr`, `device.hostname` — see
the well-known attributes discussion in *The model* above and
[zipline#105](https://github.com/org-zpr/zl-zipline/issues/105)); pruned
otherwise. It declares no identity attributes, so the identity-vendor
retention rule (ZPL.md) never applies.

### Compiled form

`policy.capnp` gains one struct and one field:

```capnp
struct TrustedService {
  # ... existing fields @0-@4 ...
  attrQuery @5 :AttrQueryConfig;   # populated only when api = "zpr-attr/1"
}

struct AttrQueryConfig {
  url            @0 :Text;
  caCertPem      @1 :Text;    # "" = system roots
  timeoutSeconds @2 :UInt32;
}
```

Mirrored in `zl-zpr-common` as `TrustedService.attr_query: Option<AttrQueryConfig>`;
an unset pointer decodes as `None`, exactly as `oidc` does today. The bearer
token the visa service presents is **not** in the policy — see *Configuration*.

---

## The wire protocol

Every endpoint lives under the policy's `url`. All requests and responses are
`application/json`, UTF-8. TLS is mandatory and is verified against the
policy's `ca_cert_path` alone when one is pinned, otherwise against system
roots. Every request carries

```
Authorization: Bearer <token>
```

The token identifies the caller and, for a hosted multi-tenant service such as
zipline, the tenant. **The protocol has no tenant parameter**: a service that
serves several ZPRnets keys them off the credential, never off anything in the
request body, so a visa service cannot ask about another tenant's actors by
naming it.

Redirects are not followed. Response bodies are read under a 1 MiB cap. The
visa service does not retry within a request; the refresh path retries
naturally on the actor's next visa request.

### `POST {url}/query`

Request:

```json
{
  "identities": {
    "user.sub": "10769150350006150715113082367",
    "user.zpr.authority": "google",
    "device.zpr.adapter.cn": "laptop.zpr.org"
  },
  "attributes": ["dept", "roles", "contractor"]
}
```

| Field | Meaning |
|---|---|
| `identities` | The actor's lookup-identity set as **ZPR key names**, exactly what `lookup_identities` produces: every policy-declared identity attribute the actor carries, plus `user.zpr.authority` when it has one. Values are single strings — a multi-valued attribute can never name an identity. This is the same shape as the top two levels of a `file` store's JSON, so a service can key its records precisely as a file store does. `user.zpr.authority` is always included when present because identities such as an OIDC `sub` are unique only within their issuer; a service serving actors from two providers must scope on it. |
| `attributes` | The service-side names from `returns_attributes` — the only names the visa service will keep. A hint: the service **may** use it to avoid fetching what will be discarded, and **may** ignore it. The visa service filters the response through the mapping regardless. |

Response, `200 OK`:

```json
{
  "attributes": {
    "dept":       { "values": ["eng"] },
    "roles":      { "values": ["a", "b"], "expires_at": "2026-09-22T15:00:00Z" },
    "contractor": { "values": [] }
  }
}
```

| Field | Meaning |
|---|---|
| `attributes` | An object keyed by service-side attribute name. Names not in the policy mapping are ignored. An unknown actor is `{}` — a **successful, empty** answer, not an error, matching the file store. |
| `values` | Required. An array of strings. For a **single-valued** mapping it must hold exactly one element. For a **multi-valued** mapping, any number. For a **tag**, the name's presence is the tag and `values` is ignored (`[]` is conventional). |
| `expires_at` | Optional, RFC 3339. When this attribute stops being true as far as the service knows. The visa service uses `min(expires_at, now + expiration_seconds)`: **policy can shorten a lifetime but never extend one** (SECURITY_MODEL.md). Absent means the policy default. A value already in the past makes the attribute absent. |

How the visa service reads the response, in order:

1. Anything but `200` with a JSON body, a body over the cap, or a body that
   does not parse to the shape above is an **error**. The actor becomes
   indeterminate and the pending visa is denied. In particular a `404` means
   the URL is wrong; it never means "unknown actor".
2. Each returned name is mapped through `returns_attributes`. Unmapped names
   are dropped silently (they were never going to become attributes).
3. A single-valued mapping whose `values` has more than one element is an
   **error** for the whole response: the service and the policy disagree about
   what the attribute *is*, and picking the first element would make an
   authorisation depend on the service's iteration order. (The `file` store is
   lenient here and takes the first; that is a pre-existing divergence, out of
   scope for this document.)
4. A single- or multi-valued mapping with empty `values` is **absent** — the
   service is saying it has no value — and the attribute is pruned from the
   actor like any attribute a source stopped vending.
5. Expiry is clamped as above; the attribute is stamped with this service's
   id as its source.

Status codes a service uses:

| Code | Meaning |
|---|---|
| `200` | Answer follows, possibly `{"attributes": {}}`. |
| `400` | Malformed request. |
| `401` | Missing or unknown bearer token. |
| `403` | Token valid, but not permitted to query. |
| `409` | **Conflict.** The identities named resolve to records that disagree on some attribute's value. The service must not pick a winner; this is the fail-closed rule of the store trait, moved to the side that can see the records. |
| `5xx` | Service failure. |

Everything other than `200` is treated identically by the visa service — deny,
log the status, retry on the next request — but the distinction is worth
making for operators reading the service's logs.

### `GET {url}/schema`

Request has no body. Response, `200 OK`:

```json
{
  "identityKeys": ["user.sub", "device.zpr.adapter.cn"],
  "attributes": [
    { "name": "dept",       "type": "string",  "multiValued": false,
      "description": "Cost centre", "canonicalValues": ["eng", "sales", "ops"] },
    { "name": "roles",      "type": "string",  "multiValued": true },
    { "name": "contractor", "type": "boolean", "description": "Not an employee" }
  ]
}
```

The `attributes` array holds **SCIM 2.0 attribute definitions** (RFC 7643,
section 7). SCIM is the directory world's vocabulary for describing user and
device attributes, and Okta, Entra and most identity products already emit
it, so a service fronting a SCIM directory can copy the `attributes` of its
own `Schema` resource here — minus `complex` ones — and a future SCIM-backed
attribute service maps onto this endpoint without translation. The envelope
around the array is ours, because SCIM has no notion of a store that is keyed
on identities *someone else* vends.

| Field | Meaning |
|---|---|
| `identityKeys` | Ours, not SCIM. The ZPR identity keys this service can look up on. Empty or absent means "unspecified". |
| `attributes[].name` | SCIM. A service-side name, as it would appear on the left of `returns_attributes`. |
| `attributes[].type` | SCIM. `string`, `boolean`, `integer`, `decimal`, `dateTime`, `reference` or `binary`. `complex` is not supported and is reported as a mismatch. |
| `attributes[].multiValued` | SCIM. Default `false`. |
| `attributes[].description` | SCIM. Optional, for editors. |
| `attributes[].canonicalValues` | SCIM. Optional. The values the service will ever return for this attribute. For editors; the visa service does not check policy literals against it in `zpr-attr/1`. |
| any other SCIM field | `required`, `caseExact`, `mutability`, `returned`, `uniqueness`, `referenceTypes` may be present and are ignored. |

How a policy mapping corresponds to a definition:

| `returns_attributes` spelling | Expected definition |
|---|---|
| `dept -> user.dept` (single-valued) | any type but `boolean` or `complex`, `multiValued: false` |
| `roles -> user.role{}` (multi-valued) | any type but `boolean` or `complex`, `multiValued: true` |
| `contractor -> #user.contractor` (tag) | `type: boolean`, `multiValued: false` |

Values still travel as strings in `values` whatever the declared SCIM type;
the type is a description for editors and a consistency check, not a wire
encoding. A `boolean` attribute's *presence* is the tag; its `values` are
ignored, as under `POST {url}/query`.

Per-value constraints beyond `canonicalValues` (a pattern, a format) are
**reserved**: a later revision may allow an optional JSON Schema fragment per
attribute describing one value. JSON Schema is not used for the schema itself
because it describes JSON shapes, and what this endpoint describes is a ZPR
attribute vocabulary — single, multi and tag all travel as `values: [...]`,
so a JSON Schema of the response would erase exactly the distinction editors
need.

The schema exists mainly for **policy editors** — an online editor can check a
`returns_attributes` list as it is typed and offer `canonicalValues` as
completions. The visa service also fetches it **once, when the store is built
at policy install**. The response is **scoped to the credential that asked**:
a service that serves different views to different tokens returns the
vocabulary that token can see, which is exactly the check RFC 19 section 6
asks the compiler to make for a delegated fragment — a `/schema` call with
the fragment's own token, no new protocol. At install the visa service:

- logs a `warn` for every mapped name the schema does not list;
- logs a `warn` for every mapped name whose policy spelling (`{}`, `#`, plain)
  disagrees with the definition per the table above, including `complex`;
- logs a `warn` if `identityKeys` is non-empty and shares nothing with the
  policy's lookup-identity keys — the classic misconfiguration of keying a
  store on `user.email`;
- logs one `info` line and moves on if the endpoint is missing or fails.

**A schema disagreement never fails a policy install.** Policy is
authoritative; the schema is advice. Coupling installation to a network call
would let an attribute service outage block a policy fix.

### Change notification

STREAM is delivered by inverting the direction: the attribute service tells the
visa service that something changed, and the visa service does what it already
does when a store is flushed — re-query the affected actors, re-evaluate their
live visas, revoke the ones that no longer hold (a durable revocation
obligation and the reconciler pass, `reconcile_obligations` in
`vs/src/event_mgr.rs`; zl-zipline#289). No standing connection, no replay protocol, no new
worker.

The service calls the visa service's admin API:

```
POST /admin/services/{id}/changed
X-API-Key: <key with notify permission>
```

with one of two bodies:

| Body | Effect |
|---|---|
| `{}` | **Everything changed.** Equivalent to today's `DELETE /admin/services/{id}/cache`: the store's revision is bumped, so every actor is stale for this source and is re-queried on its next visa request; actors behind live visas are re-queried now. |
| `{"identities": [{"user.sub": "…"}, {"device.zpr.adapter.cn": "…"}]}` | **These actors changed.** Each entry is one identity pair. Every connected actor carrying that pair has its recorded revision for this source forgotten, which makes the source stale for that actor alone; the same reconcile pass follows. Other actors are untouched. |

Responses: `202` accepted (reconcile queued; the outcome is logged, as for the
cache flush), `400` malformed, `403` key lacks permission, `404` no such
trusted service.

The endpoint is gated by a new API-key permission level, **`notify`**, that
can reach this endpoint and nothing else — the same least-privilege precedent
as the `resolve` level the CoreDNS plugin uses. A `notify` key is **bound to
exactly one trusted-service id** when it is minted (`vsapikey ... --service
<id>`), and a request whose `{id}` names any other service is `403`. So a
compromised attribute service can force re-queries of its own declaration
and nothing else; it cannot flush another attribute store or an
authentication service. A `readwrite` key may name any id, as an administrator
can today with the cache flush.

Since the key is per visa service and per declaration, a multi-tenant
attribute service holds one `notify` key per (ZPRnet, declaration) it serves
and posts to that ZPRnet's visa service. There is no rate limiting in
`zpr-attr/1`; a chatty notifier costs a reconcile pass per call against its
own store, the same cost an administrator flushing that cache would incur.

### Reserved

Two endpoints are named so that no implementer takes the paths for something
else. Neither is specified, and a visa service running `zpr-attr/1` never
calls them.

- **`GET {url}/stream`** — a standing subscription (server-sent events) that
  pushes `changed` notifications to the visa service instead of requiring the
  service to hold a visa-service credential. Worth building when an attribute
  service must sit somewhere it cannot reach the admin API from, or when the
  reconcile-everything cost of the webhook is too coarse.
- **`POST {url}/satisfies`** — an attribute expression in, a list of identity
  pairs out. Needed by policy assertions ("no consultant in Finland may reach
  Payroll") and by editors that want to preview who a rule matches. Waits on
  the assertion work; the expression syntax is the open question.

A batch form of `query` (several actors per call) is also deferred: the visa
service refreshes one actor at a time today, so nothing would call it.

---

## The visa service side

### Store

`api = "zpr-attr/1"` is constructed by the factory
(`vs/src/trusted_services/factory.rs`) into an `AttrQueryStore`
(`vs/src/trusted_services/attr_query_store.rs`) implementing
`TrustedServiceInterface`. The four trait methods map directly:

| Method | Behaviour |
|---|---|
| `get_attributes_for_actor(identities)` | One `POST {url}/query`; map, validate and clamp the response as described above. |
| `flush()` | Bump the store's revision counter (an `AtomicU64`, as the OIDC store does). There is no cache to drop. |
| `current_revision()` | The counter. |
| `get_source_id()` | The trusted-service id. |

**The store holds no attribute cache.** The actor's own attribute expiry is the
cache: `refresh_expired_attributes` (`vs/src/actor_attributes.rs`) re-queries a
source only when one of its attributes has expired or is within 45 s of
expiry (`ATTRIBUTE_REFRESH_AHEAD`, [#275](https://github.com/org-zpr/zl-zipline/issues/275)),
or its revision moved, and the visa request path consults it only then. A snapshot cache in the store
would be a second TTL to reason about with nothing to buy.

One `reqwest::Client` per store, built the way `KeySource` in
`vs/src/oidc/jwks.rs` builds its JWKS client: no redirects, the pinned roots
added, the policy timeout applied. A store whose `url` or pins change across a
policy install is rebuilt; `TrustedServiceDefinition` already compares the
whole policy record.

### Configuration

The bearer token lives with the visa service, never in the signed policy:

```toml
[core]
ts_secrets_dir = "secrets"    # default "."; relative paths anchor at vs.toml
```

The store reads `<ts_secrets_dir>/<service-id>.token`, trimmed of surrounding
whitespace, when it is built. A missing or empty file is a
`TrustedServiceInit` error and the policy install fails, the same as a `file`
store whose JSON is absent — a store that cannot authenticate cannot answer,
and a policy naming it cannot be run. Path anchoring follows `file_ts_dir`.

**One credential per declaration, never per URL, never shared.** The token
belongs to the trusted-service *id*, and the visa service holds no other
credential toward an attribute service: it is never "root" there, only
whatever the declaring administrator was granted. Two declarations may name
the same `url` under different ids with different tokens; they are two
stores, two clients, two source stamps, and the factory must **not** reject
the pair (the OIDC factory's duplicate-issuer rule does not apply here). This
is what ZPR policy delegation (RFC 19, sections 5 and 6) requires: the parent
administrator "sets the credentials with which B's policy will access
trusted services", each delegated policy keeps its own attribute cache, and a
service that serves credential-scoped views sandboxes a delegate by what its
token can see. A delegated fragment declaring the zipline service under its
own id with its own token gets all three properties from this design without
a protocol change. Only identity verification (the OIDC provider) uses
credentials global to the visa service, as the RFC says it should.

The remaining seam is the filename: `<service-id>.token` requires a plain
filename, and a delegation scheme with hierarchical ids will need a safe
mapping. It is the same change the `file` store's `<id>.json` needs, so the
two move together when the time comes.

### Refresh, pruning and revocation

Nothing changes. An attribute service's attributes carry its id as their
source and the clamped expiry, so:

- on expiry the actor is re-queried on its next visa request, and anything the
  service no longer returns is pruned (`prune_from_source`);
- on a revision bump — a `changed` notification or a cache flush — every actor
  is re-queried, and the reconcile pass re-checks and revokes live visas;
- on error the actor is indeterminate and denied, and the source stays stale
  so the next request tries again (`REVISION_NEVER`).

The new `changed` endpoint with identities adds one manager method,
`TrustedServicesMgr::forget_source_revision(zpr_addr, source)`, the per-source
sibling of `forget_actor_revisions`.

---

## Security

Threat model, in the terms of SECURITY_MODEL.md: the attribute service is a
**trusted source**, so its answers are as trustworthy as it is and no more;
the controls below are about making sure the visa service is talking to the
service policy named, that nobody else can, and that a failure anywhere
denies rather than permits.

- **Transport.** TLS is required; the compiler rejects a non-`https` URL. The
  optional CA pin travels inside the signed policy, so changing it needs the
  policy signing key, and it is exclusive: with a pin set the built-in roots
  are disabled, so "pinned" means pinned and not "also trusted". Redirects
  are refused: a redirect is a way to move a request to a host the pin does
  not cover.
- **Caller authentication.** A per-declaration bearer token, read from a file
  the visa service operator controls. It is never written to the policy, never
  logged, and never sent anywhere but the pinned `url`. There is no
  visa-service-wide credential to an attribute service; see *Configuration*
  for why that matters under delegation.
- **Callback authentication.** The `changed` endpoint requires a visa-service
  API key with the `notify` permission, which cannot read or change anything
  and is bound to one trusted-service id, so the blast radius of a leaked key
  is one store's reconcile passes.
- **Bounded work.** Every call has a timeout (≤ 30 s) and a body cap (1 MiB).
  A slow or verbose service can deny its own actors' visas — the fail-closed
  outcome — but cannot hold a request worker indefinitely.
- **Fail closed everywhere.** Transport error, bad status, malformed body,
  type disagreement on a single-valued attribute, `409` from the service: all
  deny. The only non-denying outcomes are `200` with a well-formed body.
- **Provenance intact.** The store stamps every attribute with its own id and
  can neither mint nor displace `user.zpr.authority`. A compromised attribute
  service can lie about `dept`; it cannot claim to have authenticated anyone.
- **No values in logs above `debug`.** Attribute values are personal data in
  most deployments.

**Departure from RFC-13.1.** SECURITY_MODEL.md records the RFC's design for
trusted-service calls: each signed with an HMAC over the function name, an RFC
3339 timestamp and the canonically serialised arguments. `zpr-attr/1` does not
adopt it. Over TLS to a pinned endpoint, a bearer token proves the caller and
the channel proves integrity and freshness; the HMAC adds replay and
canonicalisation machinery that every third-party implementer would have to get
exactly right, which is the opposite of what a reference API is for. Should a
deployment need the visa service to talk to an attribute service over a channel
it does not trust, the right answer is the reserved `service` property and an
on-net path, not a second signature. No implemented trusted-service API uses
the RFC's HMAC.

---

## Design decisions

Rationale carried over from the completed master plan, which was retired once
shipped (umbrella [zipline#72](https://github.com/org-zpr/zl-zipline/issues/72)).
Full plan text is in git history:
`git show a35b224:docs/plans/2026-09-22-attr-query.md`. Decisions already
argued in the body above — the bearer token over the RFC-13.1 HMAC, SCIM for
the schema, schema advice never failing an install, one credential per
declaration, the inverted STREAM — are not repeated here.

**HTTPS + JSON, not Cap'n Proto RPC or gRPC.** Anyone can implement it, and
the visa service already speaks HTTPS/JSON on its admin API. Cap'n Proto RPC
and gRPC were rejected as hostile to third-party implementers.
([#72](https://github.com/org-zpr/zl-zipline/issues/72))

**A new `api` value instead of the RFC's network trusted-service API.** That
API (`provider` tuples, fabric names, `cert_path`, HMAC-signed calls to a
ZPR-internal service) is the wrong shape for a hosted database, was never
implemented, and the compiler no longer accepts it. `zpr-attr/1` names the contract and its
version, leaving room for `zpr-attr/2`.
([#72](https://github.com/org-zpr/zl-zipline/issues/72))

**ZPR key names on the wire.** `identities` uses `user.sub`,
`device.zpr.adapter.cn`, `user.zpr.authority` as-is: it mirrors the `file`
store's JSON, needs no reverse mapping, and the authority marker has no
service-side name to map to.
([#72](https://github.com/org-zpr/zl-zipline/issues/72))

**No new store abstraction.** `TrustedServiceInterface` and
`TrustedServicesMgr` already fit a networked source, so the store is one
module plus one factory arm; refactoring bootstrap or the OIDC store onto
something else was out of scope.
([#78](https://github.com/org-zpr/zl-zipline/issues/78))

**A reference server before the first real implementer.** zipline had no code
when the protocol was fixed, so the API is the contract zipline builds to, not
a fit to its database. `zpr-attr-server` exists as a second implementation to
keep the spec honest; this document stays normative over it and over the
OpenAPI rendering. ([#80](https://github.com/org-zpr/zl-zipline/issues/80))

---

## Implementation status

**Implemented** as of 2026-09-22, by zipline issues
[#74](https://github.com/org-zpr/zl-zipline/issues/74)–[#81](https://github.com/org-zpr/zl-zipline/issues/81)
under umbrella [#72](https://github.com/org-zpr/zl-zipline/issues/72), with
these version floors:

- Policy schema: `AttrQueryConfig` in `zl-zpr-policy/policy.capnp`
  ([#74](https://github.com/org-zpr/zl-zipline/issues/74)).
- `zpr` crate (`zl-zpr-common`) **v0.29.0**: Rust mirror types and the
  submodule pointer bump
  ([#75](https://github.com/org-zpr/zl-zipline/issues/75)).
- Compiler (`zl-zpr-compiler`) **0.19.0**: `api = "zpr-attr/1"` accepted and
  woven ([#76](https://github.com/org-zpr/zl-zipline/issues/76)).
- `zl-zpr-core` pinned to `zpr` **v0.29.0**
  ([#77](https://github.com/org-zpr/zl-zipline/issues/77)).
- Visa service: `AttrQueryStore` behind the factory plus the `ts_secrets_dir`
  configuration key ([#78](https://github.com/org-zpr/zl-zipline/issues/78)),
  and `POST /admin/services/{id}/changed` with `Permission::Notify`
  ([#79](https://github.com/org-zpr/zl-zipline/issues/79)).
- Reference server `zpr-attr-server`, staged by `make release`
  ([#80](https://github.com/org-zpr/zl-zipline/issues/80)).
- Netns end-to-end test `zl-zpr-core/integration-test/attr-query-test.sh`
  ([#81](https://github.com/org-zpr/zl-zipline/issues/81)).

Why it was built this way is under *Design decisions*; this document and the
code are current.

Already in place and relied on, unchanged:

- The store trait `TrustedServiceInterface` and the manager
  `TrustedServicesMgr` (`vs/src/trusted_services/mod.rs`, `manager.rs`).
- Identity-keyed lookups (`lookup_identities`) and the authority ownership
  rule (`derive_user_authority`), both in `vs/src/trusted_services/mod.rs`.
- Expiry- and revision-driven refresh with pruning and the indeterminate
  outcome (`vs/src/actor_attributes.rs`).
- Flush-triggered reconcile and visa revocation
  (`DELETE /admin/services/{id}/cache` in `vs/src/admin_service.rs`,
  `reconcile_obligations` in `vs/src/event_mgr.rs`).
- The `OidcConfig` pattern for threading a per-kind configuration through
  `policy.capnp`, `zl-zpr-common`, the compiler and the factory.

Not built, and not planned in `zpr-attr/1`: `GET {url}/stream`,
`POST {url}/satisfies`, batch query, on-net reach via `service`, request rate
limiting on `changed`.

## Where the code lives

| Concern | Location |
|---|---|
| Schema | `zl-zpr-policy/policy.capnp` (`AttrQueryConfig`) |
| Rust mirror | `zl-zpr-common/src/policy_types/trusted_service.rs` |
| `.zplc` parsing | `zl-zpr-compiler/src/config/trusted_service.rs`, constant in `src/zpl.rs` |
| Store | `zl-zpr-visaservice/vs/src/trusted_services/attr_query_store.rs` |
| Factory arm, api constant | `zl-zpr-visaservice/vs/src/trusted_services/factory.rs` |
| Secrets directory | `zl-zpr-visaservice/vs/src/config.rs` (`ts_secrets_dir`) |
| `changed` endpoint, `notify` permission | `zl-zpr-visaservice/vs/src/admin_service.rs`, `admin_apikeys.rs`, `admin-http-api.txt` |
| Reference server | `zl-zpr-visaservice/zpr-attr-server/`, staged by `make release` so the netns tier can run it |
| End-to-end test | `zl-zpr-core/integration-test/attr-query-test.sh` |
| OpenAPI rendering | `docs/zpr-attr-v1.openapi.yaml` (this repository) |
