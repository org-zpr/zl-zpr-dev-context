# ZPR RFC Index

Two sources:

1. **Full set (internal)**: distributed to team members as `ZPR_RFC-<n>[.<rev>].pdf`
   and `.docx` files. The highest `<rev>` for a given `<n>` is current
   (`ZPR_RFC-8.5.pdf` supersedes `ZPR_RFC-8.pdf`). Only a subset is public.
2. **Public subset**: https://github.com/org-zpr/zl-zpr-rfcs — sources in
   `src/`, built PDFs in `pdf/`
   (https://github.com/org-zpr/zl-zpr-rfcs/tree/main/pdf). This is the
   zipline fork; the upstream original is https://github.com/org-zpr/zpr-rfcs,
   which is where newly published RFCs land first.

The PDFs have a clean text layer; `pdftotext` extracts them fine.

## Titles by number (latest revision as of 2026-08)

| RFC | Latest | Title |
|---|---|---|
| 0 | 0.1 | ZPR Working Group RFCs (index/process) |
| 1 | 1.4 | Zero-Trust Packet Routing |
| 2 | 2 | ZPR Policy Language (ZPL) — early |
| 3 | 3 | HTTPS Docking Protocol |
| 4 | 4.4 | ZPR Terminology (glossary) |
| 5 | 5.1 | IP Connections to a ZPR Network |
| 6 | 6.4 | ZPR Data Protocols |
| 7 | 7 | Topology, Address Assignment and Forwarding |
| 8 | 8.5 | ZPR Policy Language (ZPL) |
| 9 | 9 | Threat Resistance to Bad Actors with Access |
| 10 | 10.2 | Zero-Trust Packet Routing |
| 11 | 11 | ZPR and the Future of Zero Trust |
| 12 | 12.6 | Overview of Zero-trust Packet Routing |
| 13 | 13.1 | Authentication, Identity and Attributes |
| 14 | 14.1 | The Reasoning Behind Visas |
| 15 | 15.5 | Overview of the ZPL Policy Language |
| 16 | 16.1 | ZPR's Concept of Identity |
| 17 | 17 | ZDP Protocol Definition |
| 19 | src only | ZPR Policy Delegation (source in `zl-zpr-rfcs/src`) |

## Suggested reading order

1. RFC 12 — the overview; what ZPR is and why.
2. RFC 4 — terminology; settles what "visa", "actor", "substrate" etc. mean.
3. RFC 15 (then 8 for depth) — the ZPL policy language.
4. RFC 16 / 13 — identity, authentication, attributes.
5. RFC 14 — why visas exist; useful before touching `libeval` or `vs`.
6. RFC 6 / 17 — data protocols and ZDP; needed for link/forwarding work.
7. RFC 7 — topology, address assignment, forwarding.
