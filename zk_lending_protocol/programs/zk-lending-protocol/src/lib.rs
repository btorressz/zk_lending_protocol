use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("N36WGuo9LKUWeDBCKPcmrW8ykCgECxQsMqxzaVdzQmg");

#[program]
pub mod zk_lending_protocol {
    use super::*;

    /// Initialize the global protocol state.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let protocol_state = &mut ctx.accounts.protocol_state;
        protocol_state.total_collateral = 0;
        protocol_state.total_loans = 0;
        protocol_state.total_liquidity = 0;
        protocol_state.base_interest_rate = 5; // e.g., 5%
        protocol_state.utilization_rate = 0;
        Ok(())
    }

    /// Stake collateral with ZK-SNARK proof verification.
    /// Supports multiple collateral types by using a dedicated CollateralPool account.
    pub fn stake_collateral(
        ctx: Context<StakeCollateral>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        // Replace the dummy proof verification with your actual ZK verifier.
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        // Transfer collateral tokens from the user to the collateral pool escrow.
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_collateral_account.to_account_info(),
            to: ctx.accounts.collateral_pool_token_account.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Update the borrower's confidential collateral amount (encrypted).
        let borrower_account = &mut ctx.accounts.borrower_account;
        borrower_account.encrypted_collateral = update_encrypted_value(
            borrower_account.encrypted_collateral.clone(),
            amount,
            true,
        );

        // Update the collateral pool state.
        let collateral_pool = &mut ctx.accounts.collateral_pool;
        collateral_pool.total_collateral = collateral_pool
            .total_collateral
            .checked_add(amount)
            .ok_or(ZKError::MathOverflow)?;
        Ok(())
    }

    /// Borrow funds against staked collateral using a ZK proof to verify collateral sufficiency.
    pub fn borrow(
        ctx: Context<Borrow>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        // Check that the borrower’s encrypted collateral (via a ZK proof) is sufficient.
        require!(
            has_sufficient_collateral(
                ctx.accounts.borrower_account.encrypted_collateral.clone(),
                amount
            ),
            ZKError::InsufficientCollateral
        );

        // Transfer borrowed tokens from the lending pool escrow to the borrower.
        let cpi_accounts = Transfer {
            from: ctx.accounts.lending_pool_token_account.to_account_info(),
            to: ctx.accounts.user_borrow_token_account.to_account_info(),
            authority: ctx.accounts.lending_pool_authority.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Update the borrower’s encrypted borrowed amount.
        let borrower_account = &mut ctx.accounts.borrower_account;
        borrower_account.encrypted_borrowed = update_encrypted_value(
            borrower_account.encrypted_borrowed.clone(),
            amount,
            true,
        );

        // Update protocol state: total loans and liquidity.
        let protocol_state = &mut ctx.accounts.protocol_state;
        protocol_state.total_loans = protocol_state
            .total_loans
            .checked_add(amount)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.total_liquidity = protocol_state
            .total_liquidity
            .checked_sub(amount)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.utilization_rate =
            calculate_utilization(protocol_state.total_loans, protocol_state.total_liquidity);

        Ok(())
    }

    /// Repay borrowed funds. If fully repaid, collateral may eventually be unlocked.
    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        // Transfer tokens from the borrower back to the lending pool escrow.
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_borrow_token_account.to_account_info(),
            to: ctx.accounts.lending_pool_token_account.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Update the borrower's encrypted borrowed amount.
        let borrower_account = &mut ctx.accounts.borrower_account;
        require!(
            has_sufficient_borrow(borrower_account.encrypted_borrowed.clone(), amount),
            ZKError::RepayExceedsBorrow
        );
        borrower_account.encrypted_borrowed = update_encrypted_value(
            borrower_account.encrypted_borrowed.clone(),
            amount,
            false,
        );

        // Update protocol state.
        let protocol_state = &mut ctx.accounts.protocol_state;
        protocol_state.total_loans = protocol_state
            .total_loans
            .checked_sub(amount)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.total_liquidity = protocol_state
            .total_liquidity
            .checked_add(amount)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.utilization_rate =
            calculate_utilization(protocol_state.total_loans, protocol_state.total_liquidity);

        Ok(())
    }

    /// Liquidate an under-collateralized position using a ZK proof.
    /// This anti-front-running mechanism hides the sensitive collateral details.
    pub fn liquidate(ctx: Context<Liquidate>, zk_proof: Vec<u8>) -> Result<()> {
        // Verify that liquidation conditions are met via a ZK proof.
        require!(verify_zk_liquidation(&zk_proof), ZKError::InvalidProof);

        // Check that collateral is insufficient (dummy check).
        require!(
            !has_sufficient_collateral(
                ctx.accounts.borrower_account.encrypted_collateral.clone(),
                0
            ),
            ZKError::LiquidationNotAllowed
        );

        // Liquidate by transferring collateral value (encrypted) to the pool.
        let borrower_account = &mut ctx.accounts.borrower_account;
        let liquidated_amount = extract_value_from_encryption(borrower_account.encrypted_collateral.clone());
        borrower_account.encrypted_collateral = reset_encryption();

        let collateral_pool = &mut ctx.accounts.collateral_pool;
        collateral_pool.total_collateral = collateral_pool
            .total_collateral
            .checked_sub(liquidated_amount)
            .ok_or(ZKError::MathOverflow)?;
        Ok(())
    }

    /// Governance: Propose a protocol parameter change.
    pub fn propose_change(
        ctx: Context<ProposeChange>,
        proposal_type: u8,
        new_value: u64,
    ) -> Result<()> {
        let governance = &mut ctx.accounts.governance;
        governance.proposal_id = governance
            .proposal_id
            .checked_add(1)
            .ok_or(ZKError::MathOverflow)?;
        governance.proposal_type = proposal_type;
        governance.new_value = new_value;
        governance.votes = 0;
        Ok(())
    }

    /// Governance: Vote on a proposal.
    /// Only KYC’d institutional lenders can vote.
    pub fn vote(
        ctx: Context<Vote>,
        proposal_id: u64,
        vote: bool,
    ) -> Result<()> {
        // Ensure that the voter is in the institutional pool whitelist.
        require!(
            ctx.accounts.institutional_pool.zk_whitelist.contains(&ctx.accounts.voter.key()),
            ZKError::UnauthorizedVoter
        );

        let governance = &mut ctx.accounts.governance;
        require!(governance.proposal_id == proposal_id, ZKError::InvalidProposal);

        // Simplified voting mechanism.
        if vote {
            governance.votes = governance
                .votes
                .checked_add(1)
                .ok_or(ZKError::MathOverflow)?;
        } else {
            governance.votes = governance
                .votes
                .checked_sub(1)
                .ok_or(ZKError::MathOverflow)?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────
// Dummy & Helper Functions (Replace with actual ZK logic)
// ─────────────────────────────────────────────────────────────

/// Dummy ZK-SNARK verification for general operations.
fn verify_zk_proof(_zk_proof: &Vec<u8>) -> bool {
    true
}

/// Dummy verification for liquidation-specific proofs.
fn verify_zk_liquidation(_zk_proof: &Vec<u8>) -> bool {
    true
}

/// Updates an encrypted value by adding or subtracting a given amount.
/// In production, replace this with secure arithmetic on confidential values.
fn update_encrypted_value(
    current: EncryptedAmount,
    amount: u64,
    add: bool,
) -> EncryptedAmount {
    if add {
        EncryptedAmount {
            value: current.value + amount,
        }
    } else {
        EncryptedAmount {
            value: current.value.saturating_sub(amount),
        }
    }
}

/// Dummy check for sufficient collateral based on encrypted value.
fn has_sufficient_collateral(encrypted_collateral: EncryptedAmount, amount: u64) -> bool {
    encrypted_collateral.value >= amount
}

/// Dummy check to ensure the borrower has enough borrowed balance for repayment.
fn has_sufficient_borrow(encrypted_borrowed: EncryptedAmount, amount: u64) -> bool {
    encrypted_borrowed.value >= amount
}

/// Calculate the utilization rate as a percentage.
fn calculate_utilization(total_loans: u64, total_liquidity: u64) -> u8 {
    if total_liquidity == 0 {
        return 0;
    }
    ((total_loans as u128 * 100 / total_liquidity as u128) as u8)
}

/// Dummy function to extract a plain value from an encrypted amount.
fn extract_value_from_encryption(encrypted: EncryptedAmount) -> u64 {
    encrypted.value
}

/// Dummy function to reset an encrypted amount.
fn reset_encryption() -> EncryptedAmount {
    EncryptedAmount { value: 0 }
}

// ─────────────────────────────────────────────────────────────
// Data Structures & Accounts
// ─────────────────────────────────────────────────────────────

/// A dummy structure representing an encrypted amount.
/// In production, integrate with your ZK confidential token/encryption scheme.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct EncryptedAmount {
    pub value: u64,
}

/// Global protocol state.
#[account]
pub struct ProtocolState {
    pub total_collateral: u64,
    pub total_loans: u64,
    pub total_liquidity: u64,
    pub base_interest_rate: u8, // e.g., 5%
    pub utilization_rate: u8,
}

/// Lending pool state.
#[account]
pub struct LendingPool {
    pub pool_authority: Pubkey,
    pub total_liquidity: u64,
    pub base_interest_rate: u8,
    pub utilization_rate: u8,
}

/// Multi-collateral pool state.
#[account]
pub struct CollateralPool {
    pub asset_mint: Pubkey, // e.g., USDC, SOL, USDT
    pub total_collateral: u64,
}

/// Institutional lending pool state.
#[account]
pub struct InstitutionalLendingPool {
    pub pool_owner: Pubkey,       // Owner/manager of the pool
    pub total_liquidity: u64,
    pub zk_whitelist: Vec<Pubkey>, // List of KYC’d lenders
}

/// Borrower confidential account storing encrypted amounts.
#[account]
pub struct BorrowerAccount {
    pub encrypted_collateral: EncryptedAmount, // Encrypted collateral amount
    pub encrypted_borrowed: EncryptedAmount,   // Encrypted borrowed amount
}

/// Borrower reputation account (for ZK reputation system).
#[account]
pub struct BorrowerReputation {
    pub borrower: Pubkey,
    pub zk_reputation_score: u64, // Encrypted reputation score
}

/// Governance proposal account.
#[account]
pub struct Governance {
    pub proposal_id: u64,
    pub proposal_type: u8, // (1 = Change Interest, 2 = Adjust Collateral Factor, etc.)
    pub new_value: u64,
    pub votes: i64,
}

// ─────────────────────────────────────────────────────────────
// Contexts
// ─────────────────────────────────────────────────────────────

/// Context for initializing the protocol.
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = user, space = 8 + 8*3 + 1*2)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// Context for staking collateral.
#[derive(Accounts)]
pub struct StakeCollateral<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub collateral_pool: Account<'info, CollateralPool>,
    #[account(mut)]
    pub user_collateral_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub collateral_pool_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Context for borrowing funds.
#[derive(Accounts)]
pub struct Borrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: Safe as we derive authority via PDA.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Context for repaying loans.
#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: Safe as we derive authority via PDA.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Context for liquidating a borrower's position.
#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub collateral_pool: Account<'info, CollateralPool>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Context for proposing a governance change.
#[derive(Accounts)]
pub struct ProposeChange<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,
    #[account(init, payer = proposer, space = 8 + 8 + 1 + 8 + 8)]
    pub governance: Account<'info, Governance>,
    pub system_program: Program<'info, System>,
}

/// Context for voting on a governance proposal.
#[derive(Accounts)]
pub struct Vote<'info> {
    #[account(mut)]
    pub voter: Signer<'info>,
    #[account(mut)]
    pub governance: Account<'info, Governance>,
    #[account(mut)]
    pub institutional_pool: Account<'info, InstitutionalLendingPool>,
    pub system_program: Program<'info, System>,
}

#[error_code]
pub enum ZKError {
    #[msg("Invalid zero-knowledge proof provided")]
    InvalidProof,
    #[msg("Mathematical operation overflow")]
    MathOverflow,
    #[msg("Insufficient collateral provided")]
    InsufficientCollateral,
    #[msg("Not enough liquidity in the pool")]
    InsufficientLiquidity,
    #[msg("Repayment amount exceeds borrowed amount")]
    RepayExceedsBorrow,
    #[msg("Liquidation conditions are not met")]
    LiquidationNotAllowed,
    #[msg("Unauthorized voter")]
    UnauthorizedVoter,
    #[msg("Invalid proposal")]
    InvalidProposal,
    #[msg("Collateral still sufficient, liquidation not allowed")]
    CollateralSufficient,
}
