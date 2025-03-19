use anchor_lang::prelude::*;
use anchor_lang::solana_program::clock::Clock;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("N36WGuo9LKUWeDBCKPcmrW8ykCgECxQsMqxzaVdzQmg");

#[program]
pub mod zk_lending_protocol {
    use super::*;

    /// Initializes the protocol state and treasury.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let protocol_state = &mut ctx.accounts.protocol_state;
        protocol_state.total_collateral = 0;
        protocol_state.total_loans = 0;
        protocol_state.total_liquidity = 0;
        protocol_state.base_interest_rate = 5; // e.g., 5% per annum (example)
        protocol_state.utilization_rate = 0;
        protocol_state.min_collateral_lock_time = 600; // e.g., 600 seconds = 10 minutes

        let treasury = &mut ctx.accounts.protocol_treasury;
        treasury.total_fees_collected = 0;
        treasury.governance_fund = 0;
        Ok(())
    }

    /// Stake collateral into a specific collateral pool.
    pub fn stake_collateral(
        ctx: Context<StakeCollateral>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        // Validate proof (placeholder).
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        // Transfer collateral tokens from user to collateral pool escrow.
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_collateral_account.to_account_info(),
            to: ctx.accounts.collateral_pool_token_account.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Update the borrower's encrypted collateral.
        let borrower_account = &mut ctx.accounts.borrower_account;
        borrower_account.encrypted_collateral = update_encrypted_value(
            borrower_account.encrypted_collateral.clone(),
            amount,
            true,
        );

        // Update collateral pool state.
        let collateral_pool = &mut ctx.accounts.collateral_pool;
        collateral_pool.total_collateral = collateral_pool
            .total_collateral
            .checked_add(amount)
            .ok_or(ZKError::MathOverflow)?;
        Ok(())
    }

    /// Normal borrowing instruction with flash loan protection and treasury fee collection.
    pub fn borrow(
        ctx: Context<Borrow>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        // Verify ZK proof.
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        let clock = Clock::get()?;
        let now = clock.unix_timestamp;
        let borrower_account = &mut ctx.accounts.borrower_account;
        let protocol_state = &mut ctx.accounts.protocol_state;

        // Flash loan protection: if already borrowed, require minimum lock time.
        if borrower_account.borrow_timestamp > 0 {
            require!(
                now - borrower_account.borrow_timestamp >= protocol_state.min_collateral_lock_time,
                ZKError::CollateralLockTimeNotMet
            );
        }
        // Set the borrow timestamp.
        borrower_account.borrow_timestamp = now;

        // Check encrypted collateral sufficiency.
        require!(
            has_sufficient_collateral(
                borrower_account.encrypted_collateral.clone(),
                amount
            ),
            ZKError::InsufficientCollateral
        );

        // Deduct a borrow fee (e.g., 1%).
        let fee = amount.checked_div(100).ok_or(ZKError::MathOverflow)?;
        let net_amount = amount.checked_sub(fee).ok_or(ZKError::MathOverflow)?;

        // Transfer tokens from lending pool escrow to borrower.
        let cpi_accounts = Transfer {
            from: ctx.accounts.lending_pool_token_account.to_account_info(),
            to: ctx.accounts.user_borrow_token_account.to_account_info(),
            authority: ctx.accounts.lending_pool_authority.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            net_amount,
        )?;

        // Update treasury with collected fee.
        let treasury = &mut ctx.accounts.protocol_treasury;
        treasury.total_fees_collected = treasury
            .total_fees_collected
            .checked_add(fee)
            .ok_or(ZKError::MathOverflow)?;

        // Update the borrower's encrypted borrowed amount.
        borrower_account.encrypted_borrowed = update_encrypted_value(
            borrower_account.encrypted_borrowed.clone(),
            amount, // principal (before fee)
            true,
        );

        // Update protocol state.
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

    /// Institutional borrowing instruction that checks a whitelist and applies a fixed interest rate.
    pub fn institutional_borrow(
        ctx: Context<InstitutionalBorrow>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        let clock = Clock::get()?;
        let now = clock.unix_timestamp;
        let borrower_account = &mut ctx.accounts.borrower_account;
        let protocol_state = &mut ctx.accounts.protocol_state;
        let institutional_pool = &ctx.accounts.institutional_pool;

        // Check that the borrower is whitelisted.
        require!(
            institutional_pool.zk_whitelist.contains(&ctx.accounts.borrower.key()),
            ZKError::UnauthorizedBorrower
        );

        // Flash loan protection.
        if borrower_account.borrow_timestamp > 0 {
            require!(
                now - borrower_account.borrow_timestamp >= protocol_state.min_collateral_lock_time,
                ZKError::CollateralLockTimeNotMet
            );
        }
        borrower_account.borrow_timestamp = now;

        // (For institutional pools, you may choose to use a fixed interest rate later.)
        require!(
            has_sufficient_collateral(
                borrower_account.encrypted_collateral.clone(),
                amount
            ),
            ZKError::InsufficientCollateral
        );

        // Deduct borrow fee.
        let fee = amount.checked_div(100).ok_or(ZKError::MathOverflow)?;
        let net_amount = amount.checked_sub(fee).ok_or(ZKError::MathOverflow)?;

        // Transfer tokens.
        let cpi_accounts = Transfer {
            from: ctx.accounts.lending_pool_token_account.to_account_info(),
            to: ctx.accounts.user_borrow_token_account.to_account_info(),
            authority: ctx.accounts.lending_pool_authority.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            net_amount,
        )?;

        // Update treasury.
        let treasury = &mut ctx.accounts.protocol_treasury;
        treasury.total_fees_collected = treasury
            .total_fees_collected
            .checked_add(fee)
            .ok_or(ZKError::MathOverflow)?;

        // Update borrower's encrypted borrowed amount.
        borrower_account.encrypted_borrowed = update_encrypted_value(
            borrower_account.encrypted_borrowed.clone(),
            amount,
            true,
        );

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

    /// Delegated borrowing for DAOs/businesses that assign a credit line.
    pub fn delegated_borrow(
        ctx: Context<DelegatedBorrow>,
        amount: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        let delegated = &ctx.accounts.delegated_borrower;
        // Check that the delegate is borrowing on behalf of the delegator.
        require!(
            delegated.delegate == ctx.accounts.borrower.key(),
            ZKError::UnauthorizedBorrower
        );
        require!(
            amount <= delegated.max_borrow_amount,
            ZKError::BorrowLimitExceeded
        );

        let clock = Clock::get()?;
        let now = clock.unix_timestamp;
        let borrower_account = &mut ctx.accounts.borrower_account;
        let protocol_state = &mut ctx.accounts.protocol_state;

        if borrower_account.borrow_timestamp > 0 {
            require!(
                now - borrower_account.borrow_timestamp >= protocol_state.min_collateral_lock_time,
                ZKError::CollateralLockTimeNotMet
            );
        }
        borrower_account.borrow_timestamp = now;

        require!(
            has_sufficient_collateral(
                borrower_account.encrypted_collateral.clone(),
                amount
            ),
            ZKError::InsufficientCollateral
        );

        let fee = amount.checked_div(100).ok_or(ZKError::MathOverflow)?;
        let net_amount = amount.checked_sub(fee).ok_or(ZKError::MathOverflow)?;

        let cpi_accounts = Transfer {
            from: ctx.accounts.lending_pool_token_account.to_account_info(),
            to: ctx.accounts.user_borrow_token_account.to_account_info(),
            authority: ctx.accounts.lending_pool_authority.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            net_amount,
        )?;

        let treasury = &mut ctx.accounts.protocol_treasury;
        treasury.total_fees_collected = treasury
            .total_fees_collected
            .checked_add(fee)
            .ok_or(ZKError::MathOverflow)?;

        borrower_account.encrypted_borrowed = update_encrypted_value(
            borrower_account.encrypted_borrowed.clone(),
            amount,
            true,
        );

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

    /// Repay borrowed funds; includes accrued interest.
    pub fn repay(ctx: Context<Repay>, amount: u64) -> Result<()> {
        let clock = Clock::get()?;
        let now = clock.unix_timestamp;

        let borrower_account = &mut ctx.accounts.borrower_account;
        let protocol_state = &mut ctx.accounts.protocol_state;
        let lending_pool = &mut ctx.accounts.lending_pool;

        // Calculate time elapsed and accrued interest.
        let time_elapsed = now.checked_sub(borrower_account.borrow_timestamp).unwrap_or(0);
        // Simplified interest calculation:
        // interest_due = principal * base_interest_rate * time_elapsed / (seconds in a year * 100)
        let principal = borrower_account.encrypted_borrowed.clone().value;
        let interest_due = principal
            .checked_mul(protocol_state.base_interest_rate as u64)
            .and_then(|v| v.checked_mul(time_elapsed as u64))
            .and_then(|v| v.checked_div(31_536_000 * 100))
            .ok_or(ZKError::MathOverflow)?;

        let total_due = principal.checked_add(interest_due).ok_or(ZKError::MathOverflow)?;
        require!(amount >= total_due, ZKError::RepayExceedsBorrow);

        // Transfer repayment tokens from borrower to lending pool.
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_borrow_token_account.to_account_info(),
            to: ctx.accounts.lending_pool_token_account.to_account_info(),
            authority: ctx.accounts.borrower.to_account_info(),
        };
        token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount,
        )?;

        // Distribute a portion of repayment as yield farming rewards (e.g., 1%).
        let reward = amount.checked_div(100).ok_or(ZKError::MathOverflow)?;
        lending_pool.lender_rewards = lending_pool
            .lender_rewards
            .checked_add(reward)
            .ok_or(ZKError::MathOverflow)?;

        // Update borrower account: clear borrowed amount and reset timestamp.
        borrower_account.encrypted_borrowed = reset_encryption();
        borrower_account.borrow_timestamp = 0;

        // Update protocol state.
        protocol_state.total_loans = protocol_state
            .total_loans
            .checked_sub(principal)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.total_liquidity = protocol_state
            .total_liquidity
            .checked_add(amount)
            .ok_or(ZKError::MathOverflow)?;
        protocol_state.utilization_rate =
            calculate_utilization(protocol_state.total_loans, protocol_state.total_liquidity);

        Ok(())
    }

    /// Partial liquidation: liquidate 50% of collateral if conditions are met.
    pub fn liquidate(ctx: Context<Liquidate>, zk_proof: Vec<u8>) -> Result<()> {
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);

        // Check that collateral is insufficient.
        require!(
            !has_sufficient_collateral(
                ctx.accounts.borrower_account.encrypted_collateral.clone(),
                0
            ),
            ZKError::LiquidationNotAllowed
        );

        let borrower_account = &mut ctx.accounts.borrower_account;
        let collateral_pool = &mut ctx.accounts.collateral_pool;

        // Partial liquidation: liquidate 50% of the collateral.
        let current_collateral = extract_value_from_encryption(borrower_account.encrypted_collateral.clone());
        let liquidate_amount = current_collateral / 2;

        borrower_account.encrypted_collateral = update_encrypted_value(
            borrower_account.encrypted_collateral.clone(),
            liquidate_amount,
            false,
        );
        collateral_pool.total_collateral = collateral_pool
            .total_collateral
            .checked_sub(liquidate_amount)
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

    /// Governance: Vote on a proposal (only allowed for authorized voters).
    pub fn vote(ctx: Context<Vote>, proposal_id: u64, vote: bool) -> Result<()> {
        require!(
            ctx.accounts.institutional_pool.zk_whitelist.contains(&ctx.accounts.voter.key()),
            ZKError::UnauthorizedVoter
        );

        let governance = &mut ctx.accounts.governance;
        require!(governance.proposal_id == proposal_id, ZKError::InvalidProposal);

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

    /// Rebalance collateral: adjust collateral without revealing details.
    pub fn rebalance_collateral(
        ctx: Context<RebalanceCollateral>,
        additional_collateral: u64,
        zk_proof: Vec<u8>,
    ) -> Result<()> {
        require!(verify_zk_proof(&zk_proof), ZKError::InvalidProof);
        let borrower_account = &mut ctx.accounts.borrower_account;
        // For simplicity, we add the additional collateral (could also support reductions).
        borrower_account.encrypted_collateral = update_encrypted_value(
            borrower_account.encrypted_collateral.clone(),
            additional_collateral,
            true,
        );
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────
// Dummy & Helper Functions (Replace with actual ZK and confidential logic)
// ─────────────────────────────────────────────────────────────

fn verify_zk_proof(_zk_proof: &Vec<u8>) -> bool {
    true
}

fn update_encrypted_value(
    current: EncryptedAmount,
    amount: u64,
    add: bool,
) -> EncryptedAmount {
    if add {
        EncryptedAmount {
            value: current.value.checked_add(amount).unwrap_or(current.value),
        }
    } else {
        EncryptedAmount {
            value: current.value.saturating_sub(amount),
        }
    }
}

fn has_sufficient_collateral(encrypted_collateral: EncryptedAmount, amount: u64) -> bool {
    encrypted_collateral.value >= amount
}

fn reset_encryption() -> EncryptedAmount {
    EncryptedAmount { value: 0 }
}

fn extract_value_from_encryption(encrypted: EncryptedAmount) -> u64 {
    encrypted.value
}

fn calculate_utilization(total_loans: u64, total_liquidity: u64) -> u8 {
    if total_liquidity == 0 {
        0
    } else {
        ((total_loans as u128 * 100 / total_liquidity as u128) as u8)
    }
}

// ─────────────────────────────────────────────────────────────
// Data Structures & Accounts
// ─────────────────────────────────────────────────────────────

/// Represents an encrypted amount (placeholder for real ZK encryption).
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
    pub base_interest_rate: u8,
    pub utilization_rate: u8,
    pub min_collateral_lock_time: i64,
}

/// Lending pool state.
#[account]
pub struct LendingPool {
    pub pool_authority: Pubkey,
    pub total_liquidity: u64,
    pub base_interest_rate: u8,
    pub utilization_rate: u8,
    pub lender_rewards: u64,
}

/// Multi-collateral pool state.
#[account]
pub struct CollateralPool {
    pub asset_mint: Pubkey,
    pub total_collateral: u64,
}

/// Institutional lending pool state.
#[account]
pub struct InstitutionalLendingPool {
    pub pool_owner: Pubkey,
    pub total_liquidity: u64,
    pub fixed_interest_rate: u8,
    pub zk_whitelist: Vec<Pubkey>,
}

/// Treasury account for collecting protocol fees.
#[account]
pub struct ProtocolTreasury {
    pub total_fees_collected: u64,
    pub governance_fund: u64,
}

/// Borrower account storing confidential collateral and borrow amounts.
#[account]
pub struct BorrowerAccount {
    pub encrypted_collateral: EncryptedAmount,
    pub encrypted_borrowed: EncryptedAmount,
    pub borrow_timestamp: i64,
}

/// Borrower reputation (for a ZK-based reputation system).
#[account]
pub struct BorrowerReputation {
    pub borrower: Pubkey,
    pub zk_reputation_score: u64,
}

/// Governance proposal.
#[account]
pub struct Governance {
    pub proposal_id: u64,
    pub proposal_type: u8,
    pub new_value: u64,
    pub votes: i64,
}

/// Delegated borrower: credit line assigned by a delegator.
#[account]
pub struct DelegatedBorrower {
    pub delegator: Pubkey,
    pub delegate: Pubkey,
    pub max_borrow_amount: u64,
}

// ─────────────────────────────────────────────────────────────
// Contexts
// ─────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = user, space = 8 + 32)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(init, payer = user, space = 8 + 16)]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>,
}

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

#[derive(Accounts)]
pub struct Borrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: PDA derived authority.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(mut)]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InstitutionalBorrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: PDA derived authority.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(mut)]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub institutional_pool: Account<'info, InstitutionalLendingPool>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DelegatedBorrow<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: PDA derived authority.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(mut)]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub delegated_borrower: Account<'info, DelegatedBorrower>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Repay<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
    #[account(mut)]
    pub lending_pool: Account<'info, LendingPool>,
    /// CHECK: PDA derived authority.
    pub lending_pool_authority: AccountInfo<'info>,
    #[account(mut)]
    pub lending_pool_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_borrow_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub protocol_state: Account<'info, ProtocolState>,
    #[account(mut)]
    pub protocol_treasury: Account<'info, ProtocolTreasury>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

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

#[derive(Accounts)]
pub struct ProposeChange<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,
    #[account(init, payer = proposer, space = 8 + 8 + 1 + 8 + 8)]
    pub governance: Account<'info, Governance>,
    pub system_program: Program<'info, System>,
}

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

#[derive(Accounts)]
pub struct RebalanceCollateral<'info> {
    #[account(mut)]
    pub borrower: Signer<'info>,
    #[account(mut)]
    pub borrower_account: Account<'info, BorrowerAccount>,
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
    #[msg("Collateral lock time has not been met for flash loan protection")]
    CollateralLockTimeNotMet,
    #[msg("Unauthorized borrower")]
    UnauthorizedBorrower,
    #[msg("Borrow amount exceeds delegated credit limit")]
    BorrowLimitExceeded,
}

