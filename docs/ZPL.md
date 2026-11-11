# ZPL — the Zero-trust Policy Language

ZPL (pronounced "zipple") expresses security policy for network communication
in terms of the **attributes of the communicators** rather than network
addresses. Policy is enforced across a ZPRnet, and because no statement names
an address, a service can move between hosts, clouds, or on-premises without
any policy change.

Read this before writing ZPL, changing the compiler, or changing how the visa
service evaluates policy.

## Authoritative sources

| Source | What it governs |
|---|---|
| **ZRFC 15** — `zl-zpr-rfcs/src/15-ZPL-Overview/body.md`, published as `pdf/15-ZPL-Overview.pdf` | The language as designed. The specification. |
| **`zl-zpr-compiler/zpl.bnf`** | The grammar **as actually implemented**, reconciled against `lex.rs`, `parser.rs`, `allow.rs`, `define.rs`, and `never.rs`. Its inline comments mark every divergence from ZRFC 15. |
| **`zl-zpr-compiler/README_ZPLC.md`** | The ZPLC configuration format (TOML). |

The two disagree in places, deliberately — the compiler is still catching up.
`zpl.bnf` is the truth about what compiles today; ZRFC 15 is the truth about
where the language is going. §"Implementation status" below lists the gaps.

Related: ZRFC 4 (terminology), ZRFC 12 (ZPR overview), ZRFC 16 (identity).

---

## The model

A policy is a set of **permissions** that grant rights, plus **denials** that
override them. Any communication through a ZPRnet must be specifically allowed
by a permission; anything not permitted is denied by default.

```text
                                  trusted sources
                                  (AD, LDAP, cloud, file)
                                        │  attributes
                                        ▼
   policy.zpl  ─┐                  visa service
                ├──► zplc ──► binary policy ──► libeval ──► visa issued/denied
   policy.zplc ─┘   (signed)
   (configuration
    description)
```

**Permissions are additive.** Each statement's consequences can be understood
without reading any other statement, which is what lets independent teams
compile policy sections separately and combine the results.

**Denials are statements of intent**, written with `never`. They apply to the
combined policy of the whole ZPRnet and override any contradictory permission.
The compiler can catch some conflicts, but denials must be rechecked at
runtime because attribute values change.

### Entities

| Concept | Meaning |
|---|---|
| **ZPRnet** | The network, or group of co-managed interconnected networks, the policy governs. |
| **Device** | A real or virtual processor with a network interface: vNIC, adapter dongle, or a whole machine. At least two are in any flow. |
| **User** | An individual or authority associated with a flow. Permissions may combine user and device attributes. |
| **Service** | An application that listens for and acts on packets. At least one is in any flow. |
| **Link** | A connection between two nodes. Carries attributes (location, provider, security level, cost) declared in the configuration; constrained by `over`. |
| **Identity** | Unique within the ZPRnet, authenticated, used as the lookup token for attributes and for logging. **Policy is never written in terms of identity** — only attributes. |
| **Attribute** | A named property of an identity: a **tag** (name only), a **single-valued** attribute, or a **multi-valued** attribute. No identity has two attributes with the same name. Values are strings; digits may be read as an integer, digits with one period as a float. |
| **Trusted source** | Where attribute values come from, and the *only* place they come from. The attribute service caches them, refreshes on source-specified intervals, and honors change notifications. |
| **Circumstance** | Like an attribute, but describing the state of affairs at communication time (clock time, recent data volume). Always resolved at runtime. |

### Policy versus configuration

ZPL deliberately holds no configuration. Everything
installation-specific — static addresses (node addresses in the `.zplc`
topology; adapter addresses in a trusted service's store, granted as
`device.zpr_addr` — never in policy text, zipline#109), protocols, topology,
trusted sources, which enforcement mechanism covers which part of the
network — lives in the **configuration description** (the `.zplc` file). The
compiler combines the two to emit enforcement rules per network region.

That split is why the same policy text survives a re-addressing, a cloud
migration, or a change of enforcement mechanism.

---

## Language reference

A policy is a sequence of statements. Each begins on a new line and ends with
a period followed by a newline or end of file. **Statement order does not
matter.** Blank lines, comment-only lines, and indentation are insignificant,
and a statement may span lines — but two statements may never share a line.

```zpl
# Comments run to end of line, with # or //.
allow sales employees on managed laptops to access customer databases.
```

### Keywords

`allow`, `never`, `define`, `as`, `aka`, `with`, `to`, `access`, `on`, `over`,
`and`, `signal`, `tag`, `tags`, `optional`, `multiple`. A comma reads as `and`.

Keywords are **case-insensitive** (`ALLOW` == `allow`). `a` and `an` are
dropped wherever they appear — they exist purely so statements read as English.
All English prepositions that are not keywords are reserved for future use.
Class references are matched case-insensitively; attribute names and values are
**case-sensitive**.

### Names, strings, and values

A name is an unquoted word of Unicode alphanumerics plus `-`, `_`, and `.`, or
a quoted string. There is no leading-letter rule — `9abc` and `42` are valid
names. Dots inside an unquoted word are namespace separators
(`device.zpr.adapter.cn`).

Strings are UTF-8, in single or double quotes, and the Unicode curly variants
count as their ASCII flavor. Escapes exist only for quote characters and
backslash.

```zpl
'12-34'          a string
"12'34"          a string containing an apostrophe
1234             may be read as an integer
123.4            may be read as a float
'12\\34'         five characters, one backslash
```

An unquoted **value** is stricter than a name: it may contain a dot only when
the whole value is a decimal number with digits on both sides (`level:7`,
`ratio:123.4`).

### Attributes

```zpl
sales                                  a tag — presence or absence
department:sales                       single-valued
roles:{admin,operator}                 multi-valued
clearance:                             key-presence: the attribute exists
```

`<name>:<value>` matches a single-valued attribute equal to the value **or** a
multi-valued attribute whose set contains it. So when one statement requires two
values of a multi-valued attribute (a class plus an attribute, a subclass, or a
trailing `on` clause), the compiler requires the set to contain all of them --
the same as writing the set form. Two different values of a single-valued
attribute are a contradiction and fail with `conflicting values for attribute`.

Watch the whitespace: no space may precede the `:` (an error), and a space
*after* it silently changes the meaning — `clearance: secret` is a
key-presence followed by a separate tag `secret`, not a key/value pair.

### Classes

Predefined: `device`, `user`, `service`, and `link`. Every class name has an
auto-generated plural (`employee` → `employees`, `box` → `boxes`) and both
forms are interchangeable, so policy reads naturally either way.

New classes are defined as variants of an existing class, forming a strict
hierarchy:

```zpl
define employee as a user with an ID-number, roles and optional tags full-time, part-time, and intern.
define gateway as a service with an external-network-connection.
define internet-gateway as a gateway with external-network-connection:public-internet.
define mouse aka mice as peripheral with function:pointing.
```

A definition may add attributes, and may pin an inherited attribute to a
required value — a permission granted to `internet-gateway` then applies only
to gateways carrying that value. `aka` adds a synonym, typically an irregular
plural. Redefining a keyword, reserved word, or an existing class name, plural,
or alias is an error.

`link` is built in and **not extensible**: it cannot be a parent in a `define`,
and there are no link subclasses.

---

## Statements

### `allow` — permissions

```zpl
allow sales employees to access customer databases.
allow sales employees on managed laptops to access customer databases.
allow department:sales employees on managed laptops to access customer databases.
allow HR employees to access Timesheet-database.
```

The subject is a user, service, or device spec, optionally `on` a device spec;
the object is a service spec, optionally `on` a device spec. Within a spec the
class name may come before or after the attributes, and at most one class name
may appear — `cleared and government user`, `cleared, government user`, and
`cleared government user` are the same thing.

**`on` is positional.** Before `to access` it constrains the *accessor's*
device; after `to access` and a service clause it constrains the device of the
thing *being accessed*:

```zpl
allow sales employees to access customer databases on sales devices.
```

Each hop is permissioned separately. A load-balanced service needs both:

```zpl
allow cleared government users to access Timesheet-load-balancer.
allow Timesheet-load-balancer to access Timesheet-database.
```

### `over` — link constraints

```zpl
allow sales employees to access customer databases over secure links.
allow finance users to access payroll-services over location:usa links.
```

The statement applies only if a permitted path exists whose links *all* satisfy
the description. Only link-domain attributes are allowed here; qualifying one
with another domain (`user.foo`) is an error, and unqualified attributes
default to the link domain.

The compiler records the constraint and checks it against the declared
topology; it does **not** verify that a satisfying path exists — that is the
visa service's job at enforcement time. An `over` clause naming a link
attribute no configured link carries is an **error** (it could never match); a
clause naming a *value* no link carries, where the attribute does exist, is a
**warning** (a typo catcher that `--Werror` promotes to fatal).

At most one `over` clause per statement, before any signal clause.

### `never` — denials

```zpl
never allow internet-gateways to access internal services.
never allow role:intern users to access classified services.
never allow regulated services to access backup-services over foreign links.
```

Same shape as `allow` with `never` in front. (`never` rather than `deny`,
because it reads as English and because `deny` means different things in other
policy languages.)

### `signal` — reporting on match

```zpl
allow top-secret users to access top-secret services and signal "accessing" to Access-logger.
```

Sends the message plus the identities of every entity involved to a named
service when the communication is initiated. Only the statement that actually
allows or denies the access signals, not every statement that could have. The
signal clause comes last; nothing may follow it.

### Circumstances — **not yet implemented**

ZRFC 15 describes runtime conditions and limits:

```zpl
never allow backup:nightly servers to access backup-services before 18:00 GMT.
allow Service2 access to Service1, limited to 10Gb/day.
```

The RFC notes the syntax is not fully defined, and the compiler rejects both.

---

## The toolchain

### `zplc` — the compiler

```bash
zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the binary policy and **must match the key the visa service
is configured with**, or the visa service will reject it.

| Flag | Effect |
|---|---|
| `-k, --key <FILE>` | Private RSA key to sign the policy with |
| `-c, --config <ZPLC_FILE>` | Configuration file; defaults to the `.zpl` path with a `.zplc` extension |
| `-d, --outdir <DIR>` | Output directory (must exist) |
| `-o, --outfname <NAME>` | Output filename; defaults to the input with a `.bin2` extension |
| `-p, --parse-only` | Parse and check only; emit no binary |
| `--Werror` | Treat warnings as errors |
| `-v, --verbose` | Extra detail |

`-p` plus `--Werror` is the fast correctness check for a policy edit — it needs
no signing key.

### `zpdump` — inspect a compiled policy

```bash
zpdump path/to/policy.bin2
```

Prints the contents of a binary policy. Only the `.bin2` (v2) format is
supported.

### Configuration (`.zplc`)

TOML, documented in full in `zl-zpr-compiler/README_ZPLC.md`. The blocks, in the
suggested order:

| Block | Purpose |
|---|---|
| `[nodes.<ID>]` | Node identity, `zpr_address`, provider attributes, optional substrate addresses (a node without them may still be a link peer; the visa service is meant to fill in the address the node reports, which is not implemented yet: until [#227](https://github.com/org-zpr/zl-zipline/issues/227) such a link end gets no substrate address), and `transit = true` for a node that may carry other nodes' traffic (default false) |
| `[links.<ID>]` | Topology, plus the link `attributes` an `over` clause matches. A tag is a `#`-prefixed key with an empty value |
| `[trusted_services.<NAME>]` | Where attributes come from: `file` (local JSON), `oidc` (an OpenID Connect identity provider), or `zpr-attr/1` (networked attribute service); any other `api` is a configuration error. `default` is special — it is ZPR's own trusted service, the visa service itself verifying bootstrap (RSA) authentications against the policy's `[bootstrap]` keys |
| `[bootstrap]` | Maps a Noise CN to an RSA public key, for self-authentication before trusted services are reachable |
| `[protocols.<NAME>]` | L4 protocol and port, or ICMP type and codes |
| `[services.<NAME>]` | One per service named in the policy; `<NAME>` must match the ZPL name |

Attributes crossing this boundary are namespaced `device.`, `user.`, or
`service.`, and a trusted service maps its own names onto ZPL ones with `->`:

```toml
returns_attributes = [
  "roles -> user.role{}",     # {} marks multi-valued
  "govt  -> #user.government", # # marks a tag
  "sub   -> user.id",          # plain: single-valued
]
identity_attributes = [ "sub" ]   # service-side names, not ZPL names; oidc only
```

**A trusted service that vends identity attributes is always woven into the
compiled policy**, whether or not any ZPL statement references its returned
attributes (zipline#23). **So is a trusted service that vends a
visa-service-interpreted attribute** — `device.zpr_addr` (the static-address
grant) or `device.hostname` (the DNS hosts index), listed in
`zpl::KATTR_VS_INTERPRETED` — because the visa service reads those attributes
itself, so a ZPL reference says nothing about whether the store is needed
([zipline#105](https://github.com/org-zpr/zl-zipline/issues/105)). Every
other trusted service is retained by reference,
with one transitive exception: the weaver marks a service used when policy
names one of its attributes, then `resolve_trusted_service_providers` widens
that set to a fixpoint through *provider* attributes — a service whose
attributes appear in the provider of a retained `oidc` service's JWKS proxy is
woven too, with no direct ZPL reference.
The provider fixpoint deliberately runs *after* identity vendors are retained,
so a retained vendor's own dependencies (an OIDC JWKS proxy) still resolve
through it. Everything left outside that
closure is pruned. The reference test cannot decide the question for an
identity vendor, because its attributes are the *lookup keys* of every
attribute store in the policy — a `file` store's JSON is keyed by identity
attribute and value, and the compiler cannot see a JSON file's keys. So an
`api = "oidc"` service whose `user.sub` is referenced by nothing in ZPL must
still survive compilation: the visa service needs it to authenticate the user
and to mint the identity attribute the file store is keyed on. The compiler
emits an `info` diagnostic naming each service retained by the identity-vendor
rule (and each retained as a visa-service-interpreted vendor), and a `file` or
`zpr-attr/1` service (neither may declare identity attributes) that sits
outside the reference-plus-provider closure is still pruned as before.

A worked pair to read first: `zl-zpr-compiler/test-data/m3-ping-and-http.zpl` and
its `.zplc`. For the identity-vendor retention rule, see
`test-data/test-oidc-file-interplay.zpl` / `.zplc`; for the
visa-service-interpreted retention rule, `test-data/retain-vs-interpreted.zpl`
/ `.zplc`.

### Building and testing the compiler

```bash
make            # cargo build --all-targets
make test       # cargo test --lib && --bins && integration
make check      # fmt --check plus -D warnings on lib, zplc, zpdump
```

`test-data/rfc15-*.zpl` are the ZRFC 15 examples as compilable fixtures; the
`-todo` suffix marks the ones the compiler cannot yet accept. When adding
syntax, add the fixture there.

---

## Implementation status

Documented in ZRFC 15, **not yet in the compiler**:

- **Conditions, circumstances, and limits** — `before 18:00 GMT`,
  `limited to 10Gb/day`.
- **The `servers` class** — the predefined subclass of `devices` with a
  set-valued `services` attribute.
- **Hierarchical namespaces** — the RFC describes a `global` default namespace,
  namespaces created on first reference in a `define`, and namespace prefixes
  on quoted names. The compiler implements none of it: dots are simply kept in
  the token and the dotted string is matched verbatim, so a quoted segment can
  take neither prefix nor suffix.
- **Unquoted numerals as strings anywhere** — the RFC allows them generally;
  the compiler accepts a decimal only in value position and requires digits on
  both sides of the point.
- **Negation (`without`) and attribute sourcing (`from`)** — both are reserved
  prepositions in the lexer, not keywords. `allow users without role:intern
  to access internal services.` and `define employee as a user with multiple
  roles from ActiveDirectory.` do not compile. See
  `test-data/rfc15-006-todo.zpl` and `rfc15-012-todo.zpl`.

Also present in the compiler but not the RFC: `VisaService` / `VisaServices` is
a predefined, non-extensible service class.

Accepting the syntax and *enforcing* it are separate problems — the compiler
may parse a construct the visa service cannot yet implement.

Before assuming a construct works, check `zpl.bnf` and grep `test-data/` for a
fixture that uses it.

---

## Where the code lives

| Concern | Location |
|---|---|
| Lexer, parser | `zl-zpr-compiler/src/lex.rs`, `parser.rs` |
| Statement handling | `allow.rs`, `never.rs`, `define.rs` |
| Configuration parsing | `zl-zpr-compiler/src/config/` |
| Policy assembly and output | `weaver.rs`, `policybuilder.rs`, `policywriter.rs`, `policybinaryv2.rs` |
| Binaries | `src/bin/zplc.rs`, `src/bin/zpdump.rs` |
| Grammar | `zl-zpr-compiler/zpl.bnf` |
| Binary policy schema | `zl-zpr-policy/policy.capnp` |
| Evaluation at runtime | `zl-zpr-visaservice/libeval`, exercised by the `zpt` CLI |

A change to the language usually touches all three repositories: the grammar
and compiler in `zl-zpr-compiler`, the schema in `zl-zpr-policy`, and the evaluator
in `zl-zpr-visaservice`. See [REPOSITORIES.md](REPOSITORIES.md).
