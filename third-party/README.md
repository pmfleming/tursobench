# Dependency license provenance

No tursotui source/tests were copied or adapted. TursoBench source is new MIT code.
Dependencies retain their respective licenses.

`license-supplements/` contains verbatim upstream license/notice texts missing from
published crate archives. `sources.json` records the affected exact crate versions,
immutable upstream VCS URLs (from each crate's `.cargo_vcs_info.json`) and SHA-256.
These are license texts, not runtime source imports. Existing notices are preserved.

For egui 0.37.0, some published-crate VCS commits are unavailable upstream.
Those supplements use the upstream 0.37.0 release tag resolved to immutable commit
`841a328ba2dd8f6996beb7189ebb7fc00e668a2a`; the index records this provenance explicitly.

`python3 scripts/license-notices.py OUTPUT` collects bundled dependency license
texts (including font-specific notices), supplements and an inventory, checking
supplement hashes. It also emits `MISSING-LICENSE-TEXTS.txt`. Review that report and
license obligations before public distribution; automated collection is not a
complete legal audit. Current unresolved texts are documented in `docs/release.md`.

Maintainers can refresh supplements with `scripts/fetch-license-supplements.py`
after first generating the package's missing-text report. This maintenance tool
uses the network and must be followed by manual diff/provenance review. Normal
packaging uses the checked-in texts, not moving upstream branches.
