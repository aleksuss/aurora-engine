# In-process environment for testing Aurora EVM

`aurora-engine-sim` runs the contracts with
[`near-contract-sim`](https://github.com/near/near-contract-sim), nearcore's own runtime
inside the test process. It replaced `aurora-engine-workspace`, which had the same API but
ran a `neard` sandbox through `near-workspaces`; a test written for that crate only changes
the crate it imports:

```rust
use aurora_engine_sim::{EngineContract, EngineContractBuilder, types::NearToken};
```

Every call is still a signed transaction, and every receipt it spawns runs, callbacks and
refunds included, with nearcore's gas. There is no node to start, no network and no sandbox
binary, and a test is deterministic.

## Differences from `aurora-engine-workspace`

- `transact()` returns once the transaction and every receipt it spawned have run, refunds
  included. `Node::skip_blocks` still produces blocks for code that waits on the height.
- Accounts are written into the simulated chain directly, so any id can be the root, and
  keys are derived from the account id (`SecretKey::from_seed`).
- `Node::with_sim` hands out the `near_contract_sim::Sim` itself: the clock, raw state,
  stubs, injected faults and receipt trees. It replaces `Node::worker`.
- `Node::account(id, secret_key)` opens a handle on an existing account, and
  `RawContract::from_account` replaces `RawContract::from_contract`.
- `ExecutionOutcome` reports `block_height` rather than `block_hash`, and nearcore's
  `CryptoHash` and `AccountId`.
- A transaction that doesn't run is an `Err` from `transact()`, worded as `near-workspaces`
  words it: `Failed to query access key` when the first transaction a key signs finds no
  such key, and `unable to broadcast the transaction to the network` when the chain refuses
  one, a deleted or function-call key's included.

## Building and testing

The crate is a member of the root workspace, so `cargo make clippy` lints it and
`cargo make test-workspace` runs its tests, after building the integration-test engine WASM
they load. By hand:

```sh
cargo make build-test
cargo test --locked -p aurora-engine-sim
```

The first build compiles nearcore from git, which takes several minutes and needs libclang
for RocksDB. nearcore's VM allows 1000 WASM instances per process, so a large suite should
cap `RUST_TEST_THREADS`.
