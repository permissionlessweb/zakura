//! Contextual checks for Season 1 delegation bonds (v13).

use std::collections::HashMap;

use zakura_chain::transaction::StakingActionKind;

use crate::{
    service::{
        delegation::BondStatusInChain,
        finalized_state::{disk_format::DelegationBond, ZakuraDb},
        non_finalized_state::Chain,
    },
    SemanticallyVerifiedBlock, ValidateContextError,
};

/// Validate staking actions against the bond table.
pub fn validate_delegation_bonds(
    semantically_verified: &SemanticallyVerifiedBlock,
    non_finalized_chain: &Chain,
    finalized_state: &ZakuraDb,
) -> Result<(), ValidateContextError> {
    let mut block_new_bonds = HashMap::new();
    let mut block_unbonding_bonds = HashMap::new();

    for transaction in &semantically_verified.block.transactions {
        let Some(staking_action) = transaction.staking_action() else {
            continue;
        };
        let bond_key = staking_action.arg32_0;

        match staking_action.kind {
            StakingActionKind::CreateNewDelegationBond => {
                if block_new_bonds.contains_key(&bond_key)
                    || non_finalized_chain.delegation_bonds.contains_key(&bond_key)
                    || finalized_state.delegation_bond(&bond_key).is_some()
                {
                    return Err(ValidateContextError::InvalidDelegationBond(format!(
                        "delegation bond already exists: {bond_key:?}"
                    )));
                }
                let amount = zakura_chain::amount::Amount::try_from(staking_action.amount_zats)
                    .map_err(|e| {
                        ValidateContextError::InvalidDelegationBond(format!(
                            "invalid bond amount: {e:?}"
                        ))
                    })?;
                block_new_bonds.insert(
                    bond_key,
                    DelegationBond::new(
                        amount,
                        staking_action.arg32_2,
                        crate::TransactionLocation::from_usize(semantically_verified.height, 0),
                    ),
                );
            }
            StakingActionKind::BeginDelegationUnbonding => {
                if block_unbonding_bonds.contains_key(&bond_key) {
                    return Err(ValidateContextError::InvalidDelegationBond(format!(
                        "delegation bond already unbonding in block: {bond_key:?}"
                    )));
                }
                if block_new_bonds.contains_key(&bond_key) {
                    block_unbonding_bonds.insert(bond_key, ());
                    continue;
                }
                if let Some((_bond, status)) = non_finalized_chain.delegation_bonds.get(&bond_key) {
                    match status {
                        BondStatusInChain::Active => {
                            block_unbonding_bonds.insert(bond_key, ());
                            continue;
                        }
                        other => {
                            return Err(ValidateContextError::InvalidDelegationBond(format!(
                                "delegation bond not active ({other:?}): {bond_key:?}"
                            )));
                        }
                    }
                }
                if finalized_state.delegation_bond(&bond_key).is_some() {
                    if finalized_state.is_bond_active(&bond_key) {
                        block_unbonding_bonds.insert(bond_key, ());
                        continue;
                    }
                    return Err(ValidateContextError::InvalidDelegationBond(format!(
                        "delegation bond is not active: {bond_key:?}"
                    )));
                }
                return Err(ValidateContextError::InvalidDelegationBond(format!(
                    "delegation bond not found: {bond_key:?}"
                )));
            }
            StakingActionKind::WithdrawDelegationBond => {
                if let Some((bond, status)) = non_finalized_chain.delegation_bonds.get(&bond_key) {
                    match status {
                        BondStatusInChain::Unbonding => {
                            let bond_amount = bond.amount.zatoshis() as u64;
                            if staking_action.amount_zats != bond_amount {
                                return Err(ValidateContextError::InvalidDelegationBond(format!(
                                    "withdrawal amount {} does not match bond amount {bond_amount}: {bond_key:?}",
                                    staking_action.amount_zats
                                )));
                            }
                        }
                        BondStatusInChain::Active => {
                            return Err(ValidateContextError::InvalidDelegationBond(format!(
                                "delegation bond must be unbonded before withdrawal: {bond_key:?}"
                            )));
                        }
                        other => {
                            return Err(ValidateContextError::InvalidDelegationBond(format!(
                                "delegation bond cannot be withdrawn ({other:?}): {bond_key:?}"
                            )));
                        }
                    }
                } else if block_unbonding_bonds.contains_key(&bond_key) {
                    // Created and unbonded in this block; amount checked against create row.
                    if let Some(bond) = block_new_bonds.get(&bond_key) {
                        if staking_action.amount_zats != bond.amount.zatoshis() as u64 {
                            return Err(ValidateContextError::InvalidDelegationBond(format!(
                                "withdrawal amount does not match new bond: {bond_key:?}"
                            )));
                        }
                    }
                } else {
                    return Err(ValidateContextError::InvalidDelegationBond(format!(
                        "delegation bond not found: {bond_key:?}"
                    )));
                }
            }
            StakingActionKind::RetargetDelegationBond => {
                if block_new_bonds.contains_key(&bond_key) {
                    continue;
                }
                if let Some((_bond, status)) = non_finalized_chain.delegation_bonds.get(&bond_key) {
                    if *status != BondStatusInChain::Active {
                        return Err(ValidateContextError::InvalidDelegationBond(format!(
                            "cannot retarget non-active bond: {bond_key:?}"
                        )));
                    }
                    continue;
                }
                if finalized_state.delegation_bond(&bond_key).is_some() {
                    if !finalized_state.is_bond_active(&bond_key) {
                        return Err(ValidateContextError::InvalidDelegationBond(format!(
                            "cannot retarget delegation bond that is not active: {bond_key:?}"
                        )));
                    }
                    continue;
                }
                return Err(ValidateContextError::InvalidDelegationBond(format!(
                    "delegation bond not found for retarget: {bond_key:?}"
                )));
            }
            _ => {}
        }
    }

    Ok(())
}
