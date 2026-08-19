//! Season 1 delegation-bond state machine (crosslink_monolith v13).
//!
//! `BeginDelegationUnbonding` moves principal bonded → unbonded using the
//! bond table (the amount is not on the unbond wire). POS issuance splits
//! `POS_BLOCK_REWARD_ZATS` across Active bonds, remainder to the largest.

use std::collections::HashMap;

use zakura_chain::{
    amount::{Amount, NonNegative},
    transaction::StakingAction,
    transaction::StakingActionKind,
    value_balance::ValueBalance,
};

use crate::{
    constants::POS_BLOCK_REWARD_ZATS,
    service::finalized_state::disk_format::{BondKey, DelegationBond, TransactionLocation},
    ValidateContextError,
};

/// In-memory bond status (v13 `BondStatusInChain`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BondStatusInChain {
    /// Active and earning POS.
    Active,
    /// Unbonding; principal sits in `staking_unbonded`.
    Unbonding,
    /// Withdrawn.
    Withdrawn,
    /// Burned (slash); not produced until slash rules land.
    Burned,
}

/// Apply one staking action to the bond table and, for unbond, to the pools.
pub fn update_chain_tip_with_delegation_bond(
    chain_value_pools: &mut ValueBalance<NonNegative>,
    delegation_bonds: &mut HashMap<BondKey, (DelegationBond, BondStatusInChain)>,
    bond_retargets: &mut Vec<HashMap<BondKey, [u8; 32]>>,
    staking_action: &StakingAction,
    transaction_location: TransactionLocation,
) -> Result<(), ValidateContextError> {
    let bond_key = staking_action.arg32_0;

    match staking_action.kind {
        StakingActionKind::CreateNewDelegationBond => {
            let amount = Amount::try_from(staking_action.amount_zats).map_err(|e| {
                ValidateContextError::InvalidDelegationBond(format!("invalid bond amount: {e:?}"))
            })?;
            let bond = DelegationBond::new(amount, staking_action.arg32_2, transaction_location);
            let previous = delegation_bonds.insert(bond_key, (bond, BondStatusInChain::Active));
            if previous.is_some() {
                return Err(ValidateContextError::InvalidDelegationBond(format!(
                    "duplicate delegation bond: {bond_key:?}"
                )));
            }
        }
        StakingActionKind::BeginDelegationUnbonding => {
            let (bond, _status) = *delegation_bonds.get(&bond_key).ok_or_else(|| {
                ValidateContextError::InvalidDelegationBond(format!(
                    "delegation bond not found: {bond_key:?}"
                ))
            })?;
            let updated = DelegationBond::new(bond.amount, bond.target_finalizer, transaction_location);
            delegation_bonds.insert(bond_key, (updated, BondStatusInChain::Unbonding));

            let new_bonded = (chain_value_pools.staking_bonded_amount() - bond.amount).map_err(
                |e| {
                    ValidateContextError::InvalidDelegationBond(format!(
                        "staking_bonded pool underflow when unbonding: {e:?}"
                    ))
                },
            )?;
            chain_value_pools.set_staking_bonded_amount(new_bonded);

            let new_unbonded = (chain_value_pools.staking_unbonded_amount() + bond.amount)
                .map_err(|e| {
                    ValidateContextError::InvalidDelegationBond(format!(
                        "staking_unbonded pool overflow when unbonding: {e:?}"
                    ))
                })?;
            chain_value_pools.set_staking_unbonded_amount(new_unbonded);
        }
        StakingActionKind::WithdrawDelegationBond => {
            let (bond, _status) = *delegation_bonds.get(&bond_key).ok_or_else(|| {
                ValidateContextError::InvalidDelegationBond(format!(
                    "delegation bond not found: {bond_key:?}"
                ))
            })?;
            let updated = DelegationBond::new(bond.amount, bond.target_finalizer, transaction_location);
            delegation_bonds.insert(bond_key, (updated, BondStatusInChain::Withdrawn));
        }
        StakingActionKind::RetargetDelegationBond => {
            let old_target = delegation_bonds
                .get(&bond_key)
                .ok_or_else(|| {
                    ValidateContextError::InvalidDelegationBond(format!(
                        "delegation bond not found for retarget: {bond_key:?}"
                    ))
                })?
                .0
                .target_finalizer;
            let retargets_this_block = bond_retargets.last_mut().expect(
                "bond_retargets should have been initialized for this block",
            );
            retargets_this_block.entry(bond_key).or_insert(old_target);
            let (bond, _status) = delegation_bonds
                .get_mut(&bond_key)
                .expect("bond looked up above");
            bond.target_finalizer = staking_action.arg32_2;
        }
        _ => {}
    }

    Ok(())
}

/// Split `bond_reward_total` across Active bonds. Remainder goes to the
/// largest (tie: smaller `BondKey`). Caller must ensure at least one Active.
pub fn update_bonds_with_pos_issuance(
    bond_reward_total: u64,
    delegation_bonds: &mut HashMap<BondKey, (DelegationBond, BondStatusInChain)>,
) -> Vec<(BondKey, u64)> {
    let mut total_staked_zats = 0u64;
    let max_staker = {
        let mut iter = delegation_bonds
            .iter()
            .filter(|(_, (_, status))| *status == BondStatusInChain::Active);
        let first = iter.next().expect("caller checked any Active");
        let mut max_staker = *first.0;
        let mut biggest = first.1 .0.amount.zatoshis() as u64;
        total_staked_zats += biggest;
        for other in iter {
            let amt = other.1 .0.amount.zatoshis() as u64;
            total_staked_zats += amt;
            if amt > biggest || (amt == biggest && *other.0 < max_staker) {
                max_staker = *other.0;
                biggest = amt;
            }
        }
        max_staker
    };

    let mut so_far = 0u64;
    let mut reward_per_bond = Vec::new();
    for (bond_key, (bond, bond_status)) in delegation_bonds.iter_mut() {
        if *bond_status == BondStatusInChain::Active && *bond_key != max_staker {
            let mul = (bond.amount.zatoshis() as u128) * (bond_reward_total as u128);
            let reward = (mul / (total_staked_zats as u128)) as u64;
            so_far += reward;
            bond.amount = (bond.amount
                + Amount::try_from(reward as i64).expect("reward in range"))
            .expect("POS add fits");
            reward_per_bond.push((*bond_key, reward));
        }
    }

    {
        let (bond, _status) = delegation_bonds
            .get_mut(&max_staker)
            .expect("checked earlier");
        let reward = bond_reward_total - so_far;
        bond.amount = (bond.amount + Amount::try_from(reward as i64).expect("reward in range"))
            .expect("POS remainder fits");
        reward_per_bond.push((max_staker, reward));
    }

    reward_per_bond
}

/// Mint the Season 1 block reward into Active bonds and `staking_bonded`.
pub fn apply_pos_block_reward(
    chain_value_pools: &mut ValueBalance<NonNegative>,
    delegation_bonds: &mut HashMap<BondKey, (DelegationBond, BondStatusInChain)>,
) -> Vec<(BondKey, u64)> {
    if !delegation_bonds
        .values()
        .any(|(_, status)| *status == BondStatusInChain::Active)
    {
        return Vec::new();
    }
    let rewards = update_bonds_with_pos_issuance(POS_BLOCK_REWARD_ZATS, delegation_bonds);
    let total = Amount::try_from(POS_BLOCK_REWARD_ZATS as i64).expect("constant in range");
    let new_bonded = (chain_value_pools.staking_bonded_amount() + total)
        .expect("POS mint should not overflow bonded");
    chain_value_pools.set_staking_bonded_amount(new_bonded);
    rewards
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::finalized_state::disk_format::TransactionLocation;
    use zakura_chain::block::Height;

    #[test]
    fn pos_remainder_goes_to_largest_bond() {
        let loc = TransactionLocation::from_usize(Height(1), 0);
        let mut bonds = HashMap::new();
        let a = [1u8; 32];
        let b = [2u8; 32];
        bonds.insert(
            a,
            (
                DelegationBond::new(Amount::try_from(100u64).unwrap(), [0; 32], loc),
                BondStatusInChain::Active,
            ),
        );
        bonds.insert(
            b,
            (
                DelegationBond::new(Amount::try_from(300u64).unwrap(), [0; 32], loc),
                BondStatusInChain::Active,
            ),
        );
        let rewards = update_bonds_with_pos_issuance(100, &mut bonds);
        let total: u64 = rewards.iter().map(|(_, r)| r).sum();
        assert_eq!(total, 100);
        assert_eq!(bonds[&b].0.amount.zatoshis() as u64, 300 + 75);
        assert_eq!(bonds[&a].0.amount.zatoshis() as u64, 100 + 25);
    }
}
