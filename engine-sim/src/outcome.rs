//! Outcomes of transactions and views, shaped as `near-workspaces` reports them, so
//! the code that reads them doesn't change with the backend.

use aurora_engine_types::borsh::BorshDeserialize;
use near_contract_sim::{
    AccountId, CryptoHash, ExecutionStatus, FinalExecutionStatus, ReceiptOutcome, TxExecutionError,
    TxOutcome,
};
use near_gas::NearGas;
use serde::de::DeserializeOwned;
use std::fmt;

use crate::types::NearToken;

/// What a view call returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewResultDetails {
    pub result: Vec<u8>,
    pub logs: Vec<String>,
}

impl ViewResultDetails {
    pub fn json<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        serde_json::from_slice(&self.result).map_err(Into::into)
    }

    pub fn borsh<T: BorshDeserialize>(&self) -> anyhow::Result<T> {
        T::try_from_slice(&self.result).map_err(Into::into)
    }
}

/// The outcome of a transaction's conversion to a receipt, or of one receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionOutcome {
    /// The transaction's hash for its conversion, the receipt's id for a receipt.
    pub transaction_hash: CryptoHash,
    /// The block it ran in.
    pub block_height: u64,
    pub logs: Vec<String>,
    /// The receipts it created.
    pub receipt_ids: Vec<CryptoHash>,
    pub gas_burnt: NearGas,
    pub tokens_burnt: NearToken,
    /// The signer for a conversion, the receiver for a receipt.
    pub executor_id: AccountId,
    status: ExecutionStatus,
}

impl ExecutionOutcome {
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::SuccessValue(_) | ExecutionStatus::SuccessReceiptId(_)
        )
    }

    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Failure(_) | ExecutionStatus::Unknown
        )
    }
}

impl From<ReceiptOutcome> for ExecutionOutcome {
    fn from(outcome: ReceiptOutcome) -> Self {
        Self {
            transaction_hash: outcome.receipt_id,
            block_height: outcome.block_height,
            logs: outcome.logs,
            receipt_ids: outcome.receipt_ids,
            gas_burnt: NearGas::from_gas(outcome.gas_burnt.as_gas()),
            tokens_burnt: outcome.tokens_burnt,
            executor_id: outcome.receiver_id,
            status: outcome.status,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ExecutionDetails {
    transaction: ExecutionOutcome,
    receipts: Vec<ExecutionOutcome>,
}

impl ExecutionDetails {
    fn outcomes(&self) -> Vec<&ExecutionOutcome> {
        std::iter::once(&self.transaction)
            .chain(&self.receipts)
            .collect()
    }

    fn failures(&self) -> Vec<&ExecutionOutcome> {
        std::iter::once(&self.transaction)
            .chain(&self.receipts)
            .filter(|outcome| outcome.is_failure())
            .collect()
    }

    fn receipt_failures(&self) -> Vec<&ExecutionOutcome> {
        self.receipts
            .iter()
            .filter(|outcome| outcome.is_failure())
            .collect()
    }

    fn logs(&self) -> Vec<&str> {
        std::iter::once(&self.transaction)
            .chain(&self.receipts)
            .flat_map(|outcome| &outcome.logs)
            .map(String::as_str)
            .collect()
    }
}

/// A transaction that ran to the end of its result path.
#[derive(Clone, PartialEq, Eq)]
#[must_use = "use `into_result()` to handle potential execution errors"]
pub struct ExecutionFinalResult {
    /// Gas burnt by the conversion and every receipt.
    pub total_gas_burnt: NearGas,
    status: FinalExecutionStatus,
    details: ExecutionDetails,
}

impl fmt::Debug for ExecutionFinalResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionFinalResult")
            .field("total_gas_burnt", &self.total_gas_burnt)
            .field("transaction", &self.details.transaction)
            .field("receipts", &self.details.receipts)
            .field("status", &self.status)
            .finish()
    }
}

impl ExecutionFinalResult {
    /// Converts what the simulator settled. A transaction refused before it ran is
    /// an error, worded as `near-workspaces` reports a failed `broadcast_tx_commit`.
    pub(crate) fn from_outcome(outcome: TxOutcome) -> anyhow::Result<Self> {
        let total_gas_burnt = NearGas::from_gas(outcome.gas_burnt().as_gas());
        let TxOutcome {
            hash,
            transaction,
            receipts,
            status,
            ..
        } = outcome;
        let status = match status {
            FinalExecutionStatus::Failure(TxExecutionError::InvalidTxError(e)) => {
                anyhow::bail!(
                    "unable to broadcast the transaction to the network: transaction {hash} is invalid: {e}"
                )
            }
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                anyhow::bail!("Transaction {hash} hasn't finished")
            }
            status => status,
        };
        let transaction = transaction
            .ok_or_else(|| anyhow::anyhow!("Transaction {hash} has no outcome"))?
            .into();
        let receipts = receipts.into_iter().map(Into::into).collect();

        Ok(Self {
            total_gas_burnt,
            status,
            details: ExecutionDetails {
                transaction,
                receipts,
            },
        })
    }

    #[allow(clippy::result_large_err)] // The signature `near-workspaces` has.
    pub fn into_result(self) -> Result<ExecutionSuccess, ExecutionFailure> {
        match self.status {
            FinalExecutionStatus::SuccessValue(value) => Ok(Execution {
                total_gas_burnt: self.total_gas_burnt,
                value,
                details: self.details,
            }),
            FinalExecutionStatus::Failure(error) => Err(Execution {
                total_gas_burnt: self.total_gas_burnt,
                value: error,
                details: self.details,
            }),
            // `from_outcome` keeps only the statuses that end a result path.
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => unreachable!(),
        }
    }

    // Not `#[must_use]`, as in `near-workspaces`: tests call it for the panic alone.
    #[allow(clippy::must_use_candidate)]
    pub fn unwrap(self) -> ExecutionSuccess {
        self.into_result().unwrap()
    }

    pub fn json<T: DeserializeOwned>(self) -> anyhow::Result<T> {
        let success = self.into_result()?;
        // A method that returns nothing gives an empty value; say so rather than
        // `EOF while parsing`.
        if success.value.is_empty() {
            anyhow::bail!(
                "the function call returned an empty value, which cannot be parsed as JSON"
            );
        }
        success.json()
    }

    pub fn borsh<T: BorshDeserialize>(self) -> anyhow::Result<T> {
        self.into_result()?.borsh()
    }

    pub fn raw_bytes(self) -> anyhow::Result<Vec<u8>> {
        Ok(self.into_result()?.value)
    }

    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self.status, FinalExecutionStatus::SuccessValue(_))
    }

    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self.status, FinalExecutionStatus::Failure(_))
    }

    /// The outcome of the transaction's conversion to a receipt.
    #[must_use]
    pub const fn outcome(&self) -> &ExecutionOutcome {
        &self.details.transaction
    }

    #[must_use]
    pub fn outcomes(&self) -> Vec<&ExecutionOutcome> {
        self.details.outcomes()
    }

    #[must_use]
    pub fn receipt_outcomes(&self) -> &[ExecutionOutcome] {
        &self.details.receipts
    }

    #[must_use]
    pub fn failures(&self) -> Vec<&ExecutionOutcome> {
        self.details.failures()
    }

    #[must_use]
    pub fn receipt_failures(&self) -> Vec<&ExecutionOutcome> {
        self.details.receipt_failures()
    }

    #[must_use]
    pub fn logs(&self) -> Vec<&str> {
        self.details.logs()
    }
}

/// A finished transaction holding `T`: the bytes it returned for an
/// [`ExecutionSuccess`], the error it failed with for an [`ExecutionFailure`].
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Execution<T> {
    /// Gas burnt by the conversion and every receipt.
    pub total_gas_burnt: NearGas,
    value: T,
    details: ExecutionDetails,
}

pub type ExecutionSuccess = Execution<Vec<u8>>;
pub type ExecutionFailure = Execution<TxExecutionError>;

impl<T: fmt::Debug> fmt::Debug for Execution<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionResult")
            .field("total_gas_burnt", &self.total_gas_burnt)
            .field("transaction", &self.details.transaction)
            .field("receipts", &self.details.receipts)
            .field("value", &self.value)
            .finish()
    }
}

impl ExecutionSuccess {
    pub fn json<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        serde_json::from_slice(&self.value).map_err(Into::into)
    }

    pub fn borsh<T: BorshDeserialize>(&self) -> anyhow::Result<T> {
        T::try_from_slice(&self.value).map_err(Into::into)
    }

    pub fn raw_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.value.clone())
    }
}

impl<T> Execution<T> {
    /// The outcome of the transaction's conversion to a receipt.
    #[must_use]
    pub const fn outcome(&self) -> &ExecutionOutcome {
        &self.details.transaction
    }

    #[must_use]
    pub fn outcomes(&self) -> Vec<&ExecutionOutcome> {
        self.details.outcomes()
    }

    #[must_use]
    pub fn receipt_outcomes(&self) -> &[ExecutionOutcome] {
        &self.details.receipts
    }

    #[must_use]
    pub fn failures(&self) -> Vec<&ExecutionOutcome> {
        self.details.failures()
    }

    #[must_use]
    pub fn receipt_failures(&self) -> Vec<&ExecutionOutcome> {
        self.details.receipt_failures()
    }

    #[must_use]
    pub fn logs(&self) -> Vec<&str> {
        self.details.logs()
    }
}

impl std::error::Error for ExecutionFailure {}

impl fmt::Display for ExecutionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
