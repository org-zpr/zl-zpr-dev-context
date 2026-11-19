# ZPR Terminology

Definitions of the terms used when discussing ZPR and its components.

The authoritative source is section 4, "Terminology", of the ZPR RFCs
(`zl-zpr-rfcs/src/4-Terminology/body.md`). This document mirrors it so agents and
developers working in the workspace do not need the RFC checkout; when the two
disagree, the RFC is correct and this file needs updating.

The glossary is the RFC's, so it describes the design. The implementation has
one kind of ZPR component, the **Zode** (see "Implementation terms" below):
since zl-zipline#234 there is no separate adapter, and no docking session.
Entries marked *(RFC)* name a design concept the code no longer has as a
separate piece.

---

## Glossary

| Term | Definition |
|------|------------|
| Adapter | *(RFC)* Software that allows standard IP-based applications to connect to a ZPRnet through a secure docking session. In the implementation, the adapter role lives inside each Zode: there is no adapter process and no docking session. The `adapter/` directory of zl-zpr-core keeps the name for historical reasons. |
| Assertion | A declarative statement of policy intent in ZPL used to ensure permissions align with intended security goals and do not permit unintended communication. |
| Attribute | A property of an identity (user, device, or service); may be a tag, a name/value pair, or a name/multi-value set. |
| Authentication | The process of verifying the association between an identity and its device, user, or service. |
| Authentication Service | A service that validates the identities of endpoints, users, or services; multiple may exist in ZPRnet. |
| Byzantine Fault Tolerance | A method of building fault-tolerant services that can withstand component failures or compromises. |
| Certificate | A cryptographic document proving the association of a service identity with a specific endpoint and ports. |
| Circumstance | A runtime condition (e.g., time of day, data volume, recent usage) that can influence permissions. |
| Class | A defined group of entities (e.g., users, services) with shared attributes; can be subclassed. |
| Compliant Flow | A secure, policy-compliant communication path between endpoints, guided by visas. |
| Configuration | A complete, testable set of policies and network settings that define how ZPRnet operates. |
| Configuration Description | A ZPL component defining network-specific settings such as topology and IP addresses. |
| Device Identity | An identity for a device, typically associated with a MAC address, TPM, or certificate. |
| Dock | *(RFC)* An interface on a node that connects to endpoints using IP protocols. In the implementation, a Zode's TUN interface plays this part for its own host; a Zode accepts only Zode links, never a docked adapter. |
| Endpoint | The device or interface that connects ZPR protocols to standard IP protocols or to ZPRnet (e.g., NIC, adapter); may be virtual or physical. In the implementation, the Zode's TUN interface. |
| Flow | A unidirectional communication stream between endpoints, which may be governed by policies based on identity. |
| Identity | A unique key used to retrieve attributes associated with an endpoint, user, or service. |
| Identity Attribute | A specific attribute value, such as a serial number, used to validate an identity association. |
| Incremental Compilation | The ability to independently compile and combine parts of policy for modular policy management. |
| Management Packet | An internal packet used for control-plane functions such as visa distribution. |
| MICV | Message Integrity Check Value; a cryptographic hash ensuring the integrity of a transit packet. |
| Multiple Names | User or service identities may be mapped from multiple names to a single identity. |
| Multiple Users per Endpoint | A condition where a device hosts more than one user; packet flow may be user-agnostic. |
| Name | A string used for identifying attributes, identities, or classes; may include a namespace prefix. |
| Namespace | A context in which names are defined; names in different namespaces are not equivalent. |
| Node | A ZPR component that forwards packets and enforces policy. |
| Paranoid Design Principles | Four core security principles underlying ZPR's trust-minimized network design. |
| Permission | A positive policy statement in ZPL that allows communication under certain attribute-based conditions and circumstances. |
| Port Attribute | An attribute of a service identity indicating the ports on which it communicates. |
| Service | An application that sends/receives packets; has an identity and attributes, and is bound to endpoints by port. |
| Service Identity | An identity for a service, authenticated through certificates and tied to port numbers. |
| Signal | A message triggered by a matching policy statement to notify another service (e.g., a logger). |
| Statement | A line of ZPL code defining permissions, assertions, class definitions, or other policy elements. |
| Tag | An attribute with a name but no associated value. |
| Transit Packet | An internal ZPR packet that carries data using a visa identifier instead of IP addresses. |
| Trusted Source | A verified system (e.g., LDAP, Active Directory) used to retrieve attribute values. |
| User | A person or authority with an identity and attributes; can be associated with a flow through an endpoint. |
| User Identity | An identity tied to a user, usually authenticated via credentials such as passwords or certificates. |
| Visa | A cryptographic certificate authorizing packet travel; defines authentication, permissions, and routing. |
| ZDP | ZPR Data Protocol; allows secure tunnels through IP networks to connect adapters with docks *(RFC wording)*. In the implementation, ZDP carries the links between Zodes. |
| ZPL | Zero-Trust Policy Language; a human-readable language used to define, audit, and enforce communication policies in ZPRnet, including permissions, assertions, and class definitions. |
| ZPL Compiler | The component that checks consistency of permissions and assertions and generates enforcement rules. |
| ZPR | Zero-Trust Packet Routing; a network architecture that enforces communication policies within the network. |
| ZPR Endpoint | A user, device, or service with an authenticated identity participating in communication via ZPRnet. |
| ZPRnet | A network or group of interconnected ZPR nodes that enforce communication policies using visas, compliant flows, and ZPL rules. |

---

## Implementation terms

Terms the implementation uses that the RFC glossary does not have.

| Term | Definition |
|------|------------|
| Link | A ZDP session between two Zodes over the substrate network, keyed with Noise and authenticated by the Zodes' certificates. A Zode accepts only Zode links. See `ZODE_LINKS.md`. |
| Relay | A Zode that never dials: other Zodes dial it with their initial link, and it carries the visa service's address `fd5a:5052::1` (`vs.zpr`). Started with `ph zode --relay true`. |
| Zode | The one kind of ZPR component in the implementation: a node that carries its own actor, so it forwards and enforces like a node and connects its host's applications through its TUN interface. Run by `ph zode`, the only `ph` mode since zl-zipline#234. Each Zode is either a relay or dials its relay with `--initial-link`. |

---

## Related documents

- [`SYSTEM_OVERVIEW.md`](SYSTEM_OVERVIEW.md) — how these pieces fit together.
- [`REPOSITORIES.md`](REPOSITORIES.md) — which repository implements what.
