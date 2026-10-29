use crate::transaction::{CallTransaction, ViewTransaction};
use aurora_engine_types::account_id::AccountId;
use near_contract_sim::{AccessKey, Tx};
use std::str::FromStr;

use crate::contract::RawContract;
use crate::node::{SharedChain, lock};
use crate::outcome::ExecutionFinalResult;
use crate::types::{KeyType, NearToken, SecretKey};
use aurora_engine_types::public_key::PublicKey;

#[derive(Debug, Clone)]
pub struct Account {
    id: AccountId,
    secret_key: SecretKey,
    chain: SharedChain,
}

impl Account {
    pub(crate) const fn new(id: AccountId, secret_key: SecretKey, chain: SharedChain) -> Self {
        Self {
            id,
            secret_key,
            chain,
        }
    }

    pub fn call<F: AsRef<str>>(&self, contract_id: &AccountId, function: F) -> CallTransaction {
        CallTransaction::new(self.clone(), contract_id.clone(), function.as_ref())
    }

    pub fn view<F: AsRef<str>>(&self, contract_id: &AccountId, function: F) -> ViewTransaction<'_> {
        ViewTransaction::new(&self.chain, contract_id.clone(), function.as_ref())
    }

    pub async fn deploy(&self, wasm: &[u8]) -> anyhow::Result<RawContract> {
        self.transact(&self.id, |tx| tx.deploy(wasm))?
            .into_result()?;
        Ok(RawContract::from_account(self.clone()))
    }

    #[must_use]
    pub fn id(&self) -> AccountId {
        self.id.clone()
    }

    pub async fn create_subaccount(&self, name: &str, balance: NearToken) -> anyhow::Result<Self> {
        let id: AccountId = format!("{name}.{}", self.id)
            .parse()
            .map_err(|e| anyhow::anyhow!("Parse account_id error: {e}"))?;
        let secret_key = SecretKey::from_seed(KeyType::ED25519, id.as_ref());
        let public_key = secret_key.public_key();
        self.transact(&id, |tx| {
            tx.create_account()
                .transfer(balance)
                .add_key(public_key, AccessKey::full_access())
        })?
        .into_result()?;
        // Lets `Sim::call` sign for the new account too, for tests that use `Node::with_sim`.
        lock(&self.chain).sim.set_key(&id, &secret_key);

        Ok(Self::new(id, secret_key, self.chain.clone()))
    }

    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        let pk = self.secret_key.public_key();
        PublicKey::from_str(serde_json::to_string(&pk)?.trim_matches('"'))
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    /// Signs a transaction from this account to `receiver_id` with the actions
    /// `actions` adds, and runs it with every receipt it spawns.
    pub(crate) fn transact(
        &self,
        receiver_id: &AccountId,
        actions: impl FnOnce(Tx<'_>) -> Tx<'_>,
    ) -> anyhow::Result<ExecutionFinalResult> {
        let outcome =
            lock(&self.chain).transact(&self.id, &self.secret_key, receiver_id, actions)?;
        ExecutionFinalResult::from_outcome(outcome)
    }
}
