# zk_lending_protocol


`zk-lending-protocol` is a Solana-based decentralized lending protocol built with [Anchor] It leverages zero-knowledge proofs (ZKPs) to ensure privacy and security in lending and borrowing operations. The protocol supports features such as staking collateral, borrowing, repaying loans, liquidations, and governance.

***NOTE THIS PROJECT IS TO PRACTICE MY ZK SKILLS WITH SOLANA**

## Features

- **Collateral Staking**: Stake tokens as collateral into a specific collateral pool.
- **Borrowing**: Borrow tokens against staked collateral with flash loan protection and fee collection.
- **Institutional Borrowing**: Borrow with whitelist-based access and fixed interest rates.
- **Delegated Borrowing**: Borrow on behalf of a delegator with assigned credit limits.
- **Repayment**: Repay borrowed funds, including accrued interest.
- **Liquidation**: Partial liquidation of collateral when conditions are met.
- **Governance**: Propose and vote on protocol parameter changes.
- **Rebalancing Collateral**: Adjust collateral without revealing sensitive details.

## Accounts

### Protocol Accounts

- **ProtocolState**: Stores global protocol state, including total collateral, loans, liquidity, and interest rates.
- **ProtocolTreasury**: Manages protocol fees and governance funds.
- **LendingPool**: Represents a lending pool with liquidity and utilization metrics.
- **CollateralPool**: Represents a pool for staked collateral.
- **InstitutionalLendingPool**: A lending pool for institutional borrowers with a whitelist.
- **BorrowerAccount**: Stores encrypted collateral and borrowed amounts for a borrower.
- **Governance**: Represents a governance proposal.
- **DelegatedBorrower**: Stores credit line information for delegated borrowing.

