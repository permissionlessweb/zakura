//! Delegation bond column families (Season 1 / v13 names).

use std::collections::HashMap;

use zakura_chain::amount::{Amount, NonNegative, MAX_MONEY};

use crate::{
    request::FinalizedBlock,
    service::{
        delegation::BondStatusInChain,
        finalized_state::{
            disk_db::DiskWriteBatch,
            disk_format::{BondKey, BondStatus, DelegationBond, TransactionLocation},
            zakura_db::ZakuraDb,
            TypedColumnFamily,
        },
    },
};

/// Column family name (v13).
pub const DELEGATION_BOND_BY_KEY: &str = "delegation_bond_by_key";
/// Column family name (v13).
pub const BOND_STATUS_BY_KEY: &str = "bond_status_by_key";

pub type DelegationBondByKeyCf<'cf> = TypedColumnFamily<'cf, BondKey, DelegationBond>;
pub type BondStatusByKeyCf<'cf> = TypedColumnFamily<'cf, BondKey, BondStatus>;

impl ZakuraDb {
    pub(crate) fn delegation_bond_by_key_cf(&self) -> DelegationBondByKeyCf<'_> {
        DelegationBondByKeyCf::new(&self.db, DELEGATION_BOND_BY_KEY)
            .expect("column family was created when database was created")
    }

    pub(crate) fn bond_status_by_key_cf(&self) -> BondStatusByKeyCf<'_> {
        BondStatusByKeyCf::new(&self.db, BOND_STATUS_BY_KEY)
            .expect("column family was created when database was created")
    }

    /// Bond row if present.
    pub fn delegation_bond(&self, bond_key: &BondKey) -> Option<DelegationBond> {
        self.delegation_bond_by_key_cf().zs_get(bond_key)
    }

    /// Bond status if present.
    pub fn bond_status(&self, bond_key: &BondKey) -> Option<BondStatus> {
        self.bond_status_by_key_cf().zs_get(bond_key)
    }

    /// True if the bond exists and is Active.
    pub fn is_bond_active(&self, bond_key: &BondKey) -> bool {
        self.bond_status(bond_key)
            .map(|status| status.is_active())
            .unwrap_or(false)
    }

    /// All finalized bonds for initializing a non-finalized chain.
    pub fn all_delegation_bonds(&self) -> HashMap<BondKey, (DelegationBond, BondStatusInChain)> {
        let bond_cf = self.delegation_bond_by_key_cf();
        self.bond_status_by_key_cf()
            .zs_items_in_range_ordered(..)
            .into_iter()
            .filter_map(|(key, status)| {
                let bond = bond_cf.zs_get(&key)?;
                let in_chain = match status {
                    BondStatus::Active => BondStatusInChain::Active,
                    BondStatus::Unbonding { .. } => BondStatusInChain::Unbonding,
                    BondStatus::Withdrawn { .. } => BondStatusInChain::Withdrawn,
                    BondStatus::Burned => BondStatusInChain::Burned,
                };
                Some((key, (bond, in_chain)))
            })
            .collect()
    }
}

impl DiskWriteBatch {
    /// Write bond-table updates for `finalized` using the same kinds as v13.
    pub fn prepare_delegation_bonds_batch(
        &mut self,
        db: &ZakuraDb,
        finalized: &FinalizedBlock,
    ) -> Result<(), crate::ValidateContextError> {
        use crate::service::delegation::{
            accrue_pos_on_bonds, update_chain_tip_with_delegation_bond,
        };

        let mut bonds = db.all_delegation_bonds();
        // Scratch pools: unbond subtracts principal; we persist the table here,
        // not the dummy pool. Seed so the subtraction cannot fail.
        let mut pools = ValueBalance::<NonNegative>::zero();
        pools.set_staking_bonded_amount(
            Amount::try_from(MAX_MONEY).expect("MAX_MONEY is in range"),
        );
        let mut retargets = vec![HashMap::new()];

        for (transaction_index, transaction) in finalized.block.transactions.iter().enumerate() {
            if let Some(staking_action) = transaction.staking_action() {
                let loc = TransactionLocation::from_usize(finalized.height, transaction_index);
                update_chain_tip_with_delegation_bond(
                    &mut pools,
                    &mut bonds,
                    &mut retargets,
                    staking_action,
                    loc,
                )?;
            }
        }

        // Bond rows accrue POS; do not mint into the scratch MAX_MONEY pool
        // (that panic was height 1037: 21e6 ZEC + 5 ZEC).
        accrue_pos_on_bonds(&mut bonds);

        for (key, (bond, status)) in &bonds {
            let disk_status = match status {
                BondStatusInChain::Active => BondStatus::Active,
                BondStatusInChain::Unbonding => BondStatus::Unbonding {
                    unbonded_at: bond.created_at,
                },
                BondStatusInChain::Withdrawn => BondStatus::Withdrawn {
                    withdrawn_at: bond.created_at,
                },
                BondStatusInChain::Burned => BondStatus::Burned,
            };
            let _ = db
                .delegation_bond_by_key_cf()
                .with_batch_for_writing(self)
                .zs_insert(key, bond);
            let _ = db
                .bond_status_by_key_cf()
                .with_batch_for_writing(self)
                .zs_insert(key, &disk_status);
        }

        Ok(())
    }
}

use zakura_chain::value_balance::ValueBalance;
