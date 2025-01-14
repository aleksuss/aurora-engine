use crate::utils;
use crate::utils::solidity::random::{Random, RandomConstructor};
use aurora_engine_types::H256;

#[test]
fn test_random_number_precompile() {
    let random_seed = H256::from_slice(vec![7; 32].as_slice());
    let mut signer = utils::Signer::random();
    let mut runner = utils::deploy_runner().with_random_seed(random_seed);

    let random_ctr = RandomConstructor::load();
    let nonce = signer.use_nonce();
    let random: Random = runner
        .deploy_contract(&signer.secret_key, |ctr| ctr.deploy(nonce), random_ctr)
        .into();

    let counter_value = random.random_seed(&mut runner, &mut signer);
    assert_eq!(counter_value, random_seed);
}
