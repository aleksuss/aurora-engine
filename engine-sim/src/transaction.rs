use aurora_engine_types::account_id::AccountId;
use aurora_engine_types::borsh::{self, BorshSerialize};
use near_contract_sim::Gas;
use near_gas::NearGas;
use std::future::{IntoFuture, Ready};

use crate::account::Account;
use crate::node::{SharedChain, lock};
use crate::outcome::{ExecutionFinalResult, ViewResultDetails};
use crate::types::NearToken;

/// The gas `near-workspaces` attaches to a call by default.
const DEFAULT_CALL_FN_GAS: NearGas = NearGas::from_tgas(10);
/// The gas `near-workspaces` attaches on `max_gas()`.
const MAX_GAS: NearGas = NearGas::from_tgas(1_000);

pub struct ViewTransaction<'a> {
    chain: &'a SharedChain,
    contract_id: AccountId,
    function: String,
    args: anyhow::Result<Vec<u8>>,
}

impl<'a> ViewTransaction<'a> {
    pub(crate) fn new(chain: &'a SharedChain, contract_id: AccountId, function: &str) -> Self {
        Self {
            chain,
            contract_id,
            function: function.to_owned(),
            args: Ok(Vec::new()),
        }
    }

    #[must_use]
    pub fn args(mut self, args: Vec<u8>) -> Self {
        self.args = Ok(args);
        self
    }

    #[must_use]
    pub fn args_json<U: serde::Serialize>(mut self, args: U) -> Self {
        self.args = serde_json::to_vec(&args).map_err(Into::into);
        self
    }

    #[must_use]
    pub fn args_borsh<U: BorshSerialize>(mut self, args: U) -> Self {
        self.args = borsh::to_vec(&args).map_err(Into::into);
        self
    }

    fn view(self) -> anyhow::Result<ViewResultDetails> {
        let view = lock(self.chain)
            .sim
            .view_raw(&self.contract_id, &self.function, self.args?);
        let logs = view.logs().into_iter().map(str::to_owned).collect();

        match view.result {
            Ok(result) => Ok(ViewResultDetails { result, logs }),
            Err(failure) => Err(anyhow::anyhow!(
                "View `{}` of `{}` failed: {failure}",
                self.function,
                self.contract_id
            )),
        }
    }
}

impl IntoFuture for ViewTransaction<'_> {
    type Output = anyhow::Result<ViewResultDetails>;
    type IntoFuture = Ready<Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        std::future::ready(self.view())
    }
}

pub struct CallTransaction {
    signer: Account,
    contract_id: AccountId,
    function: String,
    args: anyhow::Result<Vec<u8>>,
    gas: NearGas,
    deposit: NearToken,
}

impl CallTransaction {
    pub(crate) fn new(signer: Account, contract_id: AccountId, function: &str) -> Self {
        Self {
            signer,
            contract_id,
            function: function.to_owned(),
            args: Ok(Vec::new()),
            gas: DEFAULT_CALL_FN_GAS,
            deposit: NearToken::from_yoctonear(0),
        }
    }

    #[must_use]
    pub fn args(mut self, args: Vec<u8>) -> Self {
        self.args = Ok(args);
        self
    }

    #[must_use]
    pub fn args_json<S: serde::Serialize>(mut self, args: S) -> Self {
        self.args = serde_json::to_vec(&args).map_err(Into::into);
        self
    }

    #[must_use]
    pub fn args_borsh<B: BorshSerialize>(mut self, args: B) -> Self {
        self.args = borsh::to_vec(&args).map_err(Into::into);
        self
    }

    #[must_use]
    pub const fn gas(mut self, gas: u64) -> Self {
        self.gas = NearGas::from_gas(gas);
        self
    }

    #[must_use]
    pub const fn max_gas(mut self) -> Self {
        self.gas = MAX_GAS;
        self
    }

    #[must_use]
    pub const fn deposit(mut self, deposit: NearToken) -> Self {
        self.deposit = deposit;
        self
    }

    pub async fn transact(self) -> anyhow::Result<ExecutionFinalResult> {
        let args = self.args?;
        let gas = Gas::from_gas(self.gas.as_gas());
        self.signer.transact(&self.contract_id, |tx| {
            tx.call(&self.function)
                .args(args)
                .gas(gas)
                .deposit(self.deposit)
        })
    }
}
