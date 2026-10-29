// Environment for testing Aurora EVM, run in-process by `near-contract-sim`.
//
// The API mirrors `aurora-engine-workspace`, the `near-workspaces` wrapper it replaced,
// so tests written for that crate run unchanged. Its methods are `async` for that reason
// only: the simulator runs each call to completion before returning.
#![allow(clippy::unused_async, clippy::unused_async_trait_impl)]

use aurora_engine_types::U256;
use aurora_engine_types::account_id::AccountId;
use aurora_engine_types::parameters::connector::FungibleTokenMetadata;

use crate::node::Node;
use crate::types::NearToken;

pub use crate::contract::{EngineContract, RawContract};
pub use near_contract_sim;

pub mod account;
pub mod contract;
pub mod macros;
pub mod node;
pub mod operation;
pub mod outcome;
pub mod result;
pub mod transaction;

pub mod types {
    pub use crate::outcome::{ExecutionFinalResult, ExecutionOutcome};
    pub use near_contract_sim::{Balance as NearToken, KeyType, SecretKey};
}

const AURORA_LOCAL_CHAIN_ID: u64 = 1313161556;
const OWNER_ACCOUNT_ID: &str = "aurora.root";
const ROOT_BALANCE: NearToken = NearToken::from_near(400);
const CONTRACT_BALANCE: NearToken = NearToken::from_near(200);

#[derive(Debug)]
pub struct EngineContractBuilder {
    code: Option<Vec<u8>>,
    chain_id: [u8; 32],
    owner_id: AccountId,
    upgrade_delay_blocks: u64,
    root_balance: NearToken,
    contract_balance: NearToken,
    ft_metadata: FungibleTokenMetadata,
}

impl EngineContractBuilder {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            code: None,
            chain_id: into_chain_id(AURORA_LOCAL_CHAIN_ID),
            owner_id: OWNER_ACCOUNT_ID.parse().unwrap(),
            upgrade_delay_blocks: 1,
            root_balance: ROOT_BALANCE,
            contract_balance: CONTRACT_BALANCE,
            ft_metadata: FungibleTokenMetadata::default(),
        })
    }

    #[must_use]
    pub fn with_code(mut self, code: Vec<u8>) -> Self {
        self.code = Some(code);
        self
    }

    #[must_use]
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = into_chain_id(chain_id);
        self
    }

    pub fn with_owner_id(mut self, owner_id: &str) -> anyhow::Result<Self> {
        self.owner_id = owner_id
            .parse()
            .map_err(|e| anyhow::anyhow!("Parse account_id error: {e}"))?;
        Ok(self)
    }

    #[must_use]
    pub const fn with_upgrade_delay_blocks(mut self, upgrade_delay_blocks: u64) -> Self {
        self.upgrade_delay_blocks = upgrade_delay_blocks;
        self
    }

    #[must_use]
    pub fn with_ft_metadata(mut self, ft_metadata: FungibleTokenMetadata) -> Self {
        self.ft_metadata = ft_metadata;
        self
    }

    #[must_use]
    pub const fn with_root_balance(mut self, balance: NearToken) -> Self {
        self.root_balance = balance;
        self
    }

    #[must_use]
    pub const fn with_contract_balance(mut self, balance: NearToken) -> Self {
        self.contract_balance = balance;
        self
    }

    pub async fn deploy_and_init(self) -> anyhow::Result<EngineContract> {
        let owner_id = self.owner_id.as_ref();
        let (owner, root) = owner_id.split_once('.').unwrap_or((owner_id, owner_id));
        let node = Node::new(root, self.root_balance).await?;
        let account = if owner == root {
            node.root()
        } else {
            node.root()
                .create_subaccount(owner, self.contract_balance)
                .await?
        };
        let public_key = account.public_key()?;
        let contract = account
            .deploy(
                &self
                    .code
                    .ok_or_else(|| anyhow::anyhow!("WASM wasn't set"))?,
            )
            .await?;
        let engine = EngineContract {
            account,
            contract,
            public_key,
            node,
        };

        engine
            .new(self.chain_id, self.owner_id, self.upgrade_delay_blocks)
            .transact()
            .await
            .map_err(|e| anyhow::anyhow!("Error while initialize aurora contract: {e}"))?;

        Ok(engine)
    }
}

fn into_chain_id(value: u64) -> [u8; 32] {
    let chain_id = U256::from(value);
    chain_id.to_big_endian()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{KeyType, SecretKey};
    use aurora_engine_types::parameters::engine::{DeployErc20TokenArgs, RelayerKeyArgs};
    use near_contract_sim::ReceiptOutcome;
    use serde_json::json;

    const FT_PATH: &str = "../engine-tests/src/tests/res/fungible_token.wasm";

    #[tokio::test]
    async fn test_creating_aurora_contract() {
        let code = get_engine_code().unwrap();
        let contract = EngineContractBuilder::new()
            .unwrap()
            .with_owner_id("aurora.test.near")
            .unwrap()
            .with_code(code)
            .deploy_and_init()
            .await
            .unwrap();

        let chain_id = contract.get_chain_id().await.unwrap().result;
        assert_eq!(
            chain_id,
            U256::from_big_endian(&into_chain_id(AURORA_LOCAL_CHAIN_ID))
        );
    }

    #[tokio::test]
    async fn test_deploy_erc20_from_nep141_runs_promise_and_callback() {
        let aurora = deploy_engine().await;
        let nep141 = aurora
            .root()
            .create_subaccount("nep141", NearToken::from_near(10))
            .await
            .unwrap()
            .deploy(&std::fs::read(FT_PATH).unwrap())
            .await
            .unwrap();
        let result = nep141
            .call("new_default_meta")
            .args_json(json!({ "owner_id": aurora.root().id(), "total_supply": "1000" }))
            .transact()
            .await
            .unwrap();
        assert!(result.is_success(), "{result:?}");

        // The engine reads `ft_metadata` with a promise and deploys the ERC-20 in its callback.
        let result = aurora
            .deploy_erc20_token(DeployErc20TokenArgs::WithMetadata(nep141.id()))
            .max_gas()
            .transact()
            .await
            .unwrap();
        assert!(result.is_success());
        assert!(
            result
                .receipt_outcomes()
                .iter()
                .any(|outcome| outcome.executor_id.as_str() == nep141.id().as_ref()),
            "{:?}",
            result.receipt_outcomes()
        );
        let hash = result.outcome().transaction_hash;
        let callback_succeeded = aurora.node.with_sim(|sim| {
            sim.outcome(hash)
                .receipt("deploy_erc20_token_callback")
                .is_some_and(ReceiptOutcome::is_success)
        });
        assert!(callback_succeeded);

        let address = result.into_value();
        let mapped = aurora.get_erc20_from_nep141(nep141.id()).await.unwrap();
        assert_eq!(mapped.result, address);
    }

    #[tokio::test]
    async fn test_contract_panic_is_a_failure() {
        let aurora = deploy_engine().await;
        let manager = aurora
            .root()
            .create_subaccount("key_manager", NearToken::from_near(10))
            .await
            .unwrap();
        assert_eq!(
            aurora.node.get_balance(&manager.id()).await.unwrap(),
            NearToken::from_near(10).as_yoctonear()
        );

        let result = manager
            .call(&aurora.id(), "add_relayer_key")
            .args_json(RelayerKeyArgs {
                public_key: aurora.public_key(),
            })
            .deposit(NearToken::from_millinear(500))
            .max_gas()
            .transact()
            .await
            .unwrap();
        assert!(result.is_failure());
        assert_eq!(result.failures().len(), 1);
        let err = result.into_result().unwrap_err();
        let message = format!("{err:?}");
        assert!(
            message.contains("Smart contract panicked: ERR_KEY_MANAGER_IS_NOT_SET"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn test_missing_key_fails_like_near_workspaces() {
        let aurora = deploy_engine().await;
        let unknown = aurora.create_account(
            &"unknown.root".parse().unwrap(),
            SecretKey::from_seed(KeyType::ED25519, "unknown.root"),
        );
        // The first transaction a key signs fetches its nonce, which fails for a missing key.
        let err = unknown
            .call(&aurora.id(), "get_version")
            .transact()
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Failed to query access key"),
            "{err}"
        );

        let user = aurora
            .root()
            .create_subaccount("user", NearToken::from_near(10))
            .await
            .unwrap();
        let result = user
            .call(&aurora.id(), "get_version")
            .transact()
            .await
            .unwrap();
        assert!(result.is_success());
        aurora.node.with_sim(|sim| {
            let id = user.id();
            let public_key = SecretKey::from_seed(KeyType::ED25519, id.as_ref()).public_key();
            sim.tx(&id, &id)
                .delete_key(public_key)
                .transact()
                .assert_success();
        });
        // Later ones reuse the cached nonce, so the chain refuses the transaction.
        let err = user
            .call(&aurora.id(), "get_version")
            .transact()
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("unable to broadcast the transaction to the network"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_skip_blocks() {
        let aurora = deploy_engine().await;
        let height = aurora.node.with_sim(|sim| sim.height());

        aurora.node.skip_blocks(5).await.unwrap();

        assert_eq!(aurora.node.with_sim(|sim| sim.height()), height + 6);
    }

    async fn deploy_engine() -> EngineContract {
        EngineContractBuilder::new()
            .unwrap()
            .with_code(get_engine_code().unwrap())
            .deploy_and_init()
            .await
            .unwrap()
    }

    fn get_engine_code() -> anyhow::Result<Vec<u8>> {
        std::fs::read("../bin/aurora-engine-test.wasm").map_err(Into::into)
    }
}
