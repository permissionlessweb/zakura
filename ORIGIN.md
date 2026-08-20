# Origin

- upstream: https://github.com/zakura-core/zakura
- tag / commit: forked at v1.2.0 (4f2189a6bbe693a1283829bea8cd54abf01c2e39); rebased onto origin/main 7ff04c0f3 (2026-08-19)
- date forked: 2026-08-18
- license: MIT AND Apache-2.0 (see upstream)
- crosslink source: https://github.com/ShieldedLabs/zebra-crosslink (design + prototype port)
- status: stock PoW default; lab TFL behind zakurad `--features crosslink` and `[crosslink] enabled = true`. Tenderlink starts when listen/peers are set. Not mainnet, not ZIP, not IBC.
- retrograde: see `RETROGRADE.md` for what to drop or replace when rebasing onto current Zakura or a newer Crosslink network version.
