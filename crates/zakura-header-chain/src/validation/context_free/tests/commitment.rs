use super::super::*;
use zakura_chain::{
    block::{genesis::regtest_genesis_block, Commitment, CommitmentError},
    parameters::{
        testnet::{ConfiguredActivationHeights, Parameters},
        Network,
    },
};

#[test]
fn custom_overlapping_activations_select_the_configured_commitment_variant() {
    let activation_height = zakura_chain::block::Height(10);
    let heartwood_canopy = Parameters::build()
        .with_network_name("OverlappingCommitments")
        .expect("the custom network name is valid")
        .with_activation_heights(ConfiguredActivationHeights {
            heartwood: Some(activation_height.0),
            canopy: Some(activation_height.0),
            ..Default::default()
        })
        .expect("same-height upgrades are valid")
        .clear_funding_streams()
        .to_network()
        .expect("the custom-network parameters are valid");
    let mut header = regtest_genesis_block().header.as_ref().clone();
    header.commitment_bytes = [0; 32].into();
    assert_eq!(
        validate_commitment_structure(&header, &heartwood_canopy, activation_height),
        Ok(Commitment::ChainHistoryActivationReserved),
        "an overwritten Heartwood activation still requires its reserved value"
    );
    header.commitment_bytes = [1; 32].into();
    assert!(matches!(
        validate_commitment_structure(&header, &heartwood_canopy, activation_height),
        Err(CommitmentError::InvalidChainHistoryActivationReserved { .. })
    ));

    let through_nu5 = Parameters::build()
        .with_network_name("OverlappingNu5Commitment")
        .expect("the custom network name is valid")
        .with_activation_heights(ConfiguredActivationHeights {
            heartwood: Some(activation_height.0),
            canopy: Some(activation_height.0),
            nu5: Some(activation_height.0),
            ..Default::default()
        })
        .expect("same-height upgrades are valid")
        .clear_funding_streams()
        .to_network()
        .expect("the custom-network parameters are valid");
    assert!(matches!(
        validate_commitment_structure(&header, &through_nu5, activation_height),
        Ok(Commitment::ChainHistoryBlockTxAuthCommitment(_))
    ));
}

/// ClT0 only lists `NU6 = 1`. Block 1 keeps ZIP-221 reserved zeros in the
/// commitment field; Zakura must not recompute a NU5+ digest and reject it.
#[test]
fn custom_nu6_activation_accepts_reserved_zero_commitment() {
    let clt0 = Parameters::build()
        .with_genesis_hash("05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38")
        .expect("ClT0 genesis hash parses")
        .with_activation_heights(ConfiguredActivationHeights {
            nu6: Some(1),
            ..Default::default()
        })
        .expect("NU6-only activations are valid")
        .clear_funding_streams()
        .to_network()
        .expect("ClT0-shaped parameters are valid");
    let mut header = regtest_genesis_block().header.as_ref().clone();
    header.commitment_bytes = [0; 32].into();
    assert_eq!(
        validate_commitment_structure(&header, &clt0, zakura_chain::block::Height(1)),
        Ok(Commitment::ChainHistoryActivationReserved),
    );
    header.commitment_bytes = [0x32; 32].into();
    assert!(
        matches!(
            validate_commitment_structure(&header, &clt0, zakura_chain::block::Height(1)),
            Ok(Commitment::ChainHistoryBlockTxAuthCommitment(_))
        ),
        "non-zero bytes still take the ZIP-244 path"
    );
}

#[test]
fn custom_testnet_accepts_zip244_built_from_reserved_parent() {
    use zakura_chain::block::{
        merkle::AuthDataRoot, ChainHistoryBlockTxAuthCommitmentHash,
        ChainHistoryMmrRootHash, CHAIN_HISTORY_ACTIVATION_RESERVED,
    };

    let clt0 = Parameters::build()
        .with_genesis_hash("05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38")
        .expect("ClT0 genesis hash parses")
        .with_activation_heights(ConfiguredActivationHeights {
            nu6: Some(1),
            ..Default::default()
        })
        .expect("NU6-only activations are valid")
        .clear_funding_streams()
        .to_network()
        .expect("ClT0-shaped parameters are valid");

    let auth = AuthDataRoot::from([0x11; 32]);
    let mmr = ChainHistoryMmrRootHash::from([0x22; 32]);
    let from_tree = ChainHistoryBlockTxAuthCommitmentHash::from_commitments(&mmr, &auth);
    let from_reserved = ChainHistoryBlockTxAuthCommitmentHash::from_commitments(
        &CHAIN_HISTORY_ACTIVATION_RESERVED.into(),
        &auth,
    );
    assert_ne!(from_tree, from_reserved);
    assert!(
        from_reserved.matches_on_network(&clt0, from_tree, &auth),
        "custom testnet must accept the reserved-parent ZIP-244 digest"
    );
    assert!(
        !from_reserved.matches_on_network(&Network::new_default_testnet(), from_tree, &auth),
        "default testnet must still require the MMR parent"
    );
}
