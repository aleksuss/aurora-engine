use aurora_engine_types::account_id::AccountId;
use near_contract_sim::{PublicKey, Sim, Tx, TxOutcome};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::account::Account;
use crate::types::{KeyType, NearToken, SecretKey};

/// The chain every handle created from one [`Node`] shares.
pub(crate) type SharedChain = Arc<Mutex<Chain>>;

/// Locks the chain. A panic inside the simulator already fails the test that
/// caused it, so a poisoned lock still guards the state to read.
pub(crate) fn lock(chain: &SharedChain) -> MutexGuard<'_, Chain> {
    chain.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
pub(crate) struct Chain {
    pub(crate) sim: Sim,
    /// The access keys a handle has signed with, whose nonce `near-workspaces`'
    /// RPC client would have fetched and cached.
    queried_keys: HashSet<(AccountId, PublicKey)>,
}

impl Chain {
    /// Signs a transaction from `signer_id` to `receiver_id` with `secret_key` and the
    /// actions `actions` adds, and runs it with every receipt it spawns.
    pub(crate) fn transact(
        &mut self,
        signer_id: &AccountId,
        secret_key: &SecretKey,
        receiver_id: &AccountId,
        actions: impl FnOnce(Tx<'_>) -> Tx<'_>,
    ) -> anyhow::Result<TxOutcome> {
        self.query_access_key(signer_id, &secret_key.public_key())?;
        let tx = self.sim.tx(signer_id, receiver_id).sign_with(secret_key);
        Ok(actions(tx).transact())
    }

    /// Fails as `near-workspaces` does when the first transaction signed with
    /// `public_key` finds no such access key. Later transactions reuse the cached
    /// nonce, so a key deleted since then is the chain's to refuse.
    fn query_access_key(
        &mut self,
        account_id: &AccountId,
        public_key: &PublicKey,
    ) -> anyhow::Result<()> {
        let key = (account_id.clone(), public_key.clone());
        if self.queried_keys.contains(&key) {
            return Ok(());
        }
        if self.sim.access_key(account_id, public_key).is_none() {
            anyhow::bail!(
                "Failed to query access key: `{account_id}` has no access key `{public_key}`"
            );
        }
        self.queried_keys.insert(key);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    root: Account,
    chain: SharedChain,
}

impl Node {
    pub async fn new(root: &str, root_balance: NearToken) -> anyhow::Result<Self> {
        let root_id: AccountId = root
            .parse()
            .map_err(|e| anyhow::anyhow!("Parse account_id error: {e}"))?;
        let secret_key = SecretKey::from_seed(KeyType::ED25519, root);
        let mut sim = Sim::new();
        // The simulator writes the account straight into its genesis state, so a
        // top-level account needs no registrar, unlike in a sandbox.
        sim.create_account_with_key(root, root_balance, &secret_key);
        let chain = Arc::new(Mutex::new(Chain {
            sim,
            queried_keys: HashSet::new(),
        }));
        let root = Account::new(root_id, secret_key, chain.clone());

        Ok(Self { root, chain })
    }

    #[must_use]
    pub fn root(&self) -> Account {
        self.root.clone()
    }

    /// A handle on the existing account `account_id`, signing with `secret_key`,
    /// which must be one of the account's access keys.
    #[must_use]
    pub fn account(&self, account_id: &AccountId, secret_key: SecretKey) -> Account {
        Account::new(account_id.clone(), secret_key, self.chain.clone())
    }

    /// Runs `f` on the simulator itself, for what the handles don't cover: the
    /// clock, raw state, stubs and the receipt trees of earlier transactions.
    pub fn with_sim<R>(&self, f: impl FnOnce(&mut Sim) -> R) -> R {
        f(&mut lock(&self.chain).sim)
    }

    pub async fn get_balance(&self, account_id: &AccountId) -> anyhow::Result<u128> {
        lock(&self.chain)
            .sim
            .account(account_id)
            .map(|view| view.amount.as_yoctonear())
            .ok_or_else(|| anyhow::anyhow!("Account `{account_id}` does not exist"))
    }

    /// Produces `num_blocks` blocks after the next one, as a sandbox's
    /// `fast_forward` does.
    pub async fn skip_blocks(&self, num_blocks: u64) -> anyhow::Result<()> {
        self.with_sim(|sim| {
            let height = sim
                .height()
                .checked_add(num_blocks)
                .and_then(|h| h.checked_add(1))
                .ok_or_else(|| anyhow::anyhow!("Block height overflow"))?;
            sim.advance_to(height);
            Ok(())
        })
    }
}
