# Retrograde — ClT0 / Season 1 lab → current Zakura

This branch (`crosslink-lab`) is a **follower replica** of ShieldedLabs
**Crosslink Season 1** (`zebra-crosslink` / `crosslink_monolith` **v13**,
vendored hasher `807b995`). It syncs the live ClT0 PoW chain. It is **not**
stock `zakura-core` `main`, and it is **not** a NU7 / official-testnet node.

Use this page when the Crosslink community moves the **network** (new magic,
NU7+, new staking kinds, checkpoints) or when we rebase this lab onto a
**newer Zakura**. Each row is something we changed or punched for ClT0. To
go “forward” (newer Zakura, or a newer Crosslink net), **drop or replace**
the row; do not grow it.

SoT for Season 1 wire/state:

- `crosslink_monolith` @ `807b995` (`zebra-state` bond table, POS, unbond)
- ClT0: magic `ClT0` `[67,108,84,48]`, genesis
  `05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38`,
  NU6 @ height 1, VCrosslink v7 group `0xFFFFFFFE`

Live IBD (2026-08-20): image `217187107` /
`registry.terp.network/zakura@sha256:6a7bb37e…` climbed **past 62k** after
fixing POS mint into a dummy `MAX_MONEY` pool at height 1037.

Do **not** force-push `zakura-core` `main`. Lab lives on
`permissionlessweb/zakura` `crosslink-lab`.

---

## 1. Two different “latest”

| “Latest” | What it means | What this branch does |
|---|---|---|
| **Latest Zakura** (`zakura-core/zakura` `main`) | NU6.1+ / NU7 paths, official testnet checkpoints, Ironwood, VCT, header-chain engine, no Season 1 bond CFs | We forked ~v1.2.0 and rebased; ClT0 work sits **on top**. Rebase onto newer `main`, then **re-apply** only the rows below that still match the **network**. |
| **Latest Crosslink net** | Whatever ShieldedLabs mines next (new magic, NU7 activation, new staking kinds, TFL/slash, checkpoints) | Treat **v13 + ClT0 params** as the current net. When they ship a new net, **replace** params and hasher — do not keep ClT0 skips. |

`estimatedheight ~144k` on RPC is the **official Zcash testnet** estimator.
Ignore it for ClT0.

---

## 2. Network parameters (must match the chain, not Zakura defaults)

| Item | ClT0 / this lab | Latest Zakura / official testnet | Retrograde |
|---|---|---|---|
| Magic | `ClT0` | Official testnet magic | New net → new magic + genesis. Do not reuse ClT0 config. |
| Genesis | `05a60a92…2c38` | Official / mainnet genesis | Same. |
| Upgrade | **NU6 @ 1** only | NU6.1, NU6.2, NU6.3, NU7 heights | ClT0 has **no** official NU6.1+. Do not enable those activations on this net. A **new** Crosslink net that activates NU7 must set those heights from **their** chain, then drop §3 punches that exist only because NU6-at-1 is a lie relative to ZIP-271. |
| Checkpoints | Height 0 only | Mandatory checkpoints through Canopy+ | Official Zebra never ran uncheckpointed custom nets. Full validation from 1 is why we see nBits / maturity / ZIP-244. A mature Crosslink net should **publish checkpoints** past the experimental region, then delete the skips. |
| Coinbase maturity | v13 uses **2**; we skip 100 on custom testnet | 100 | When ClT0 (or successor) documents maturity, set the constant and **delete** the skip. |
| nBits / ZIP-208 | ClT0 advertises targets Zakura header-chain would reject | Averaging window + min-diff | Skip: “any compact target ≤ PoWLimit”. Replace with **their** difficulty law, or checkpoints. |
| Funding / deferred / miner fees | ClT0 does not pay our default-testnet streams; pre-NU6 **inequality** kept | NU6+ exact coinbase equality + lockbox | Skip funding-address match; zero deferred in miner-fee math; keep inequality. A NU7 net with a real funding split must **implement their addresses** and **restore equality**. |

---

## 3. `is_custom_testnet()` punches (delete when the check is true)

These are **not** Season 1 consensus. They exist because ClT0 was mined
under rules Zakura’s uncheckpointed path enforces and official Zebra does
not (checkpoints sit past the rule).

| Skip | File | Hides | Delete when |
|---|---|---|---|
| Funding-stream address/amount match | `zakura-consensus` `block/check.rs` | `FundingStreamNotFound` | ClT0 (or new net) pays the ZIP-271 receivers we have, **or** we load **their** address list. |
| Deferred in miner-fee total; pre-NU6 inequality | same | `InvalidMinerFees` | Coinbase actually equals subsidy+fees−deferred under NU6+. |
| nBits vs ZIP-208 / ThresholdBits | `zakura-header-chain` `validation/contextual/validate.rs` | difficulty reject | Their nBits match averaging-window mean, **or** checkpoints. |
| Transparent coinbase maturity 100 | `zakura-state` `check/utxo.rs`, `zakura-consensus` `transaction/check.rs` | spend of height-1 coinbase at 5 | Maturity is 100, **or** we set v13’s 2 as the param. |
| `hashBlockCommitments` match-all | `zakura-chain` `block/commitment.rs` | ZIP-244 commitment / auth-tree mismatch | Our auth-data merkle (including Crosslink digest policy) **equals** the header. |

Auth-data tree: v13’s empty `ZTxCrosslinkHash` node is still a **TODO** in
their `txid.rs`; we only append `ZTxCrosslinkHash` when staking is `Some`.
That is why the commitment skip is still required on ClT0. A net that
**always** appends the empty node must change `zip244.rs` **and** then drop
the skip.

---

## 4. Season 1 state that **is** the design (keep / port, do not punch)

Port these onto a newer Zakura rather than re-guessing.

| Mechanism | Where | Notes |
|---|---|---|
| VCrosslink v7 + group `0xFFFFFFFE` | `zakura-chain` transaction serialize | Stock `zcash_primitives` 0.30 cannot **read** v7. |
| Staking kinds 1–7 wire + ZIP-244 preimage | `transaction/staking.rs`, `zip244.rs` | v13 `CreateNewDelegationBond` is 169B, not old ADD/SUB. New kinds → extend `TryFrom<u8>` **and** `txid_preimage`. |
| Tx remaining value: kind 1 → bonded, kind 3 → unbonded | `transaction.rs` `staking_action_value_balance` | Matches v13. Kind 2 unbond is **not** in remaining (amount not on wire). |
| Bond table | `zakura-state` `delegation.rs`, CFs `delegation_bond_by_key`, `bond_status_by_key` | Format **minor 28.2**. Wipe on upgrade from pre-table disks. |
| Unbond: table principal bonded → unbonded | `update_chain_tip_with_delegation_bond` | v13 `zebra-state/src/service.rs`. |
| POS `500_000_000` zats/block on **Active** bonds | `POS_BLOCK_REWARD_ZATS`, `update_bonds_with_pos_issuance` | Remainder to largest / smaller `BondKey`. |
| Finalize: accrue POS on **bond rows** without minting the scratch pool | `zakura_db/delegation.rs` | **Bug:** dummy pool was `MAX_MONEY` then `apply_pos_block_reward` → panic at **1037**. Dummy path must **not** mint into that pool. Real pool still mints. |
| Fat pointer after Equihash | `zakura-chain` `fat_pointer.rs` | 44 + u16 + n×96. |
| `MAX_HEADER_BYTES` 1 MiB | `header_chain_values.rs` | Stock was 2 KiB; 334 was 2109 B. Durable form: Equihash header + `u16::MAX` sigs, or p2p message cap — not an arbitrary 2 KiB. |
| V5 projection for Orchard FFI | `transaction.rs` `to_librustzcash` | Feed v7 **body** as V5 to 0.30 FFI; native ZIP-244 for v7 **id**. Long-term: vendor/bump primitives that speak v7. |

v13 still does **not** (and we do not, for IBD):

- **Slash / `hardfork_schedule` / `burn_delegation_bonds`**
- **Kind 6** `ConvertFinalizerRewardToDelegationBond` as a table update (`_ => {}` on both)
- **TFL / Malachite as consensus** — lab TFL idle; fat pointers are header **bytes**. A **producer** needs a live roster.
- Carrying `bond_rewards` / `unbonding_amounts` on `FinalizedBlock` — we **replay** on finalize. v13 carries. Replay is two copies of the same function; prefer carrying when porting forward.

---

## 5. Library / node shape vs latest Zakura

| Piece | Lab | Latest Zakura | Retrograde |
|---|---|---|---|
| `librustzcash` / `zcash_primitives` | 0.30 + V5 projection | Track their pin; may still lack v7 | Until primitives parse v7, keep the projection **or** vendor v13’s crate. |
| Ironwood / NU6.3 | Code present; ClT0 does not activate | Official path | Leave dead on ClT0. Activate only with **their** heights. |
| VCT / header-chain engine | Present; ClT0 IBD used legacy fallback when body-sync froze | Default for mainnet-scale | Keep; not a ClT0 consensus skip. |
| RPC `valuePools` | No staking_bonded/unbonded in `getblockchaininfo` | Transparent/sprout/sapling/orchard/lockbox/ironwood | Add pools to RPC when operators need ZIP-209 vs v13. |
| Feature `crosslink` | Required for this image | Off = stock PoW | Stock `main` must stay feature-off. |

---

## 6. How to move

### A. Newer Zakura, **same** ClT0 net

1. Rebase `crosslink-lab` onto `zakura-core/zakura` `main` (no force-push of `main`).
2. Re-resolve `BondKey` / `disk_format` visibility, Amount `Result` vs `map_err`, format **minor** bump if CFs already exist upstream.
3. Keep §3 skips **only** while ClT0 headers still fail the real check.
4. Wipe state if format major/minor changes.
5. `FEATURES=crosslink` on **groot2** (amd64), pin digest, remanifest, wipe.

### B. New Crosslink network version (NU7+, new magic)

1. Copy **their** genesis, magic, activation heights, funding addresses, maturity, nBits law — from the node that **mines**, not from this file.
2. Diff staking `txid_preimage` and wire vs v13 kinds 1–7; add kinds before guessing.
3. **Delete** every `is_custom_testnet()` skip that their chain now satisfies.
4. Port slash/TFL only if **that** net finalizes with them; follower IBD may still idle TFL.
5. Prefer v13 finalize **carried** `bond_rewards` over dual replay.
6. Native v7 FFI if their primitives crate supports it; drop V5 projection.

### C. Do not

- Punch merkle/nBits/history/maturity as a long-term strategy.
- Keep follower reroute/clamp/blind POS (`d572af233`); those were reverted in spirit by the bond table + dummy-pool fix.
- Treat official-testnet `estimatedheight` as ClT0 tip.
- Remanifest onto a disk that panicked or that predates the bond CFs.

---

## 7. Commit map (lab)

| Commit | Role |
|---|---|
| `e69ac03b9` | v13 staking wire + `ZTxCrosslinkHash` |
| `27702524d` | v7 body as V5 for Orchard FFI |
| `81f225624` | staking pools in remaining tx value |
| `5e5fa4bdd` / 1 MiB later | header-chain fat pointer bound |
| `37b280db8` | bond table, unbond transfer, POS split |
| `896584a5b` | export bond types; Amount try_from |
| `217187107` | do not POS-mint the scratch `MAX_MONEY` pool |

Image pin at time of writing:
`registry.terp.network/zakura@sha256:6a7bb37e6b12c8b4c78fb03cb36f1b76b6029a73c0521e4de0a22e95833eb9c1`
(`217187107`). IBD observed through **62k+** on ClT0.
