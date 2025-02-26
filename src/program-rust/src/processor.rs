//! Program state processor

use crate::{
    error::OneSolError,
    instruction::{DexConfig, Initialize, OneSolInstruction, Swap},
    state::OneSolState,
    swappers::{token_swap::TokenSwap, Swapper},
    util::unpack_token_account,
};

use num_traits::FromPrimitive;
use solana_program::{
    account_info::{next_account_info, next_account_infos, AccountInfo},
    decode_error::DecodeError,
    entrypoint::ProgramResult,
    log::sol_log_compute_units,
    msg,
    program::invoke_signed,
    program_error::{PrintProgramError, ProgramError},
    program_pack::Pack,
    pubkey::Pubkey,
};

/// Program state handler.
pub struct Processor {}

impl Processor {
    /// Processes an [Instruction](enum.Instruction.html).
    pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], input: &[u8]) -> ProgramResult {
        let instruction = OneSolInstruction::unpack(input)?;
        match instruction {
            OneSolInstruction::Initialize(Initialize { nonce }) => {
                msg!("Instruction: Initialize");
                Self::process_initialize(program_id, nonce, accounts)
            }
            OneSolInstruction::Swap(Swap {
                amount_in,
                minimum_amount_out,
                dex_configs,
            }) => {
                msg!("Instruction: Swap");
                Self::process_swap(
                    program_id,
                    amount_in,
                    minimum_amount_out,
                    &dex_configs[..],
                    accounts,
                )
            }
        }
    }

    /// Processes initialize
    pub fn process_initialize(
        program_id: &Pubkey,
        nonce: u8,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let onesol_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let token_info = next_account_info(account_info_iter)?;
        let token_program_info = next_account_info(account_info_iter)?;

        let token_program_id = *token_program_info.key;

        if *authority_info.key != Self::authority_id(program_id, onesol_info.key, nonce)? {
            return Err(OneSolError::InvalidProgramAddress.into());
        }
        let token = unpack_token_account(token_info, &token_program_id)?;
        if token.delegate.is_some() {
            if token.delegate.unwrap() != *authority_info.key {
                return Err(OneSolError::InvalidDelegate.into());
            }
        } else if *authority_info.key != token.owner {
            return Err(OneSolError::InvalidOwner.into());
        }
        // if token.close_authority.is_some() {
        //     return Err(OneSolError::InvalidCloseAuthority.into());
        // }
        let obj = OneSolState {
            version: 1,
            nonce,
            token_program_id,
            token: *token_info.key,
            token_mint: token.mint,
        };
        OneSolState::pack(obj, &mut onesol_info.data.borrow_mut())?;
        Ok(())
    }

    /// Processes an [Swap](enum.Instruction.html).
    pub fn process_swap(
        program_id: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        dex_configs: &[DexConfig],
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        msg!("start process swap");
        if amount_in < 1 {
            return Err(OneSolError::InvalidInput.into());
        }

        let account_info_iter = &mut accounts.iter();
        let protocol_account = next_account_info(account_info_iter)?;
        let protocol_authority = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let protocol_token_account = next_account_info(account_info_iter)?;
        let source_info = next_account_info(account_info_iter)?;
        let destination_info = next_account_info(account_info_iter)?;
        let token_program_info = next_account_info(account_info_iter)?;

        if protocol_account.owner != program_id {
            return Err(ProgramError::IncorrectProgramId);
        }
        let protocol_info = OneSolState::unpack(&protocol_account.data.borrow())?;

        if *protocol_authority.key
            != Self::authority_id(program_id, protocol_account.key, protocol_info.nonce)?
        {
            return Err(OneSolError::InvalidProgramAddress.into());
        }

        if *destination_info.key == protocol_info.token || *source_info.key == protocol_info.token {
            return Err(OneSolError::IncorrectSwapAccount.into());
        }

        if *source_info.key == *destination_info.key {
            return Err(OneSolError::InvalidInput.into());
        }

        let token_program_id = *token_program_info.key;

        let protocol_token = unpack_token_account(protocol_token_account, &token_program_id)?;
        let destination_token = unpack_token_account(destination_info, &token_program_id)?;
        if protocol_token.mint != destination_token.mint {
            return Err(OneSolError::InvalidInput.into());
        }

        // if *user_transfer_authority_info.key != source_info.delegate {
        //     return Err(OneSolError::InvalidOwner.into());
        // }

        let mut swappers: Vec<TokenSwap> = vec![];

        for dex_config in dex_configs.iter() {
            #[allow(unused_parens)]
            if (dex_config.dex_type == 0) {
                let dex_accounts = next_account_infos(account_info_iter, dex_config.account_size)?;
                swappers.push(TokenSwap::new_spl_token_swap(
                    token_program_info.clone(),
                    user_transfer_authority_info.clone(),
                    source_info.clone(),
                    protocol_token_account.clone(),
                    dex_accounts,
                )?);
            }
        }

        let dest_account1 =
            spl_token::state::Account::unpack(&protocol_token_account.data.borrow())?;

        let amount1 = dest_account1.amount;

        // let (best, parts) = if swappers.len() > 1 {
        //     let _parts = find_best_parts(amount_in, swappers.len() as u64);
        //     msg!("best parts: {}", _parts);
        //     sol_log_compute_units();
        //     let _best = Self::get_expected_return_with_gas(amount_in, _parts, &swappers[..]);
        //     sol_log_compute_units();
        //     msg!("Best split is {:?}", _best);
        //     (_best, _parts)
        // } else {
        //     (vec![1], 1)
        // };

        // let mut best_index: usize = 0;
        for i in 0..swappers.len() {
            let ratio = dex_configs[i].ratio as u64;
            let token_swap_amount_in = amount_in * ratio;
            let token_swap_minimum_amount_out = minimum_amount_out * ratio;
            // best_index += 1;
            if token_swap_amount_in <= 0 {
                continue;
            }
            msg!(
                "swap onesolA -> onesolB using token-swap[{}], amount_in: {}, minimum_amount_out: {}",
                i,
                token_swap_amount_in,
                token_swap_minimum_amount_out,
            );
            swappers[i].invoke_swap(token_swap_amount_in, token_swap_minimum_amount_out)?;
        }

        let dest_account =
            spl_token::state::Account::unpack(&protocol_token_account.data.borrow())?;
        let result_amount = dest_account.amount - amount1;

        // TODO 计算手续费
        // msg!(
        //     "onesol_destination amount: {}, should tranfer: {}",
        //     dest_account.amount,
        //     result_amount,
        // );
        if result_amount < minimum_amount_out {
            return Err(OneSolError::ExceededSlippage.into());
        }
        // Transfer OnesolB -> AliceB
        msg!("transfer OneSolB -> AliceB");
        sol_log_compute_units();
        Self::token_transfer(
            protocol_account.key,
            token_program_info.clone(),
            protocol_token_account.clone(),
            destination_info.clone(),
            protocol_authority.clone(),
            protocol_info.nonce,
            result_amount,
        )
        .unwrap();

        Ok(())
    }

    /// Calculates the authority id by generating a program address.
    pub fn authority_id(
        program_id: &Pubkey,
        my_info: &Pubkey,
        nonce: u8,
    ) -> Result<Pubkey, OneSolError> {
        Pubkey::create_program_address(&[&my_info.to_bytes()[..32], &[nonce]], program_id)
            .or(Err(OneSolError::InvalidProgramAddress))
    }

    /// Issue a spl_token `Transfer` instruction.
    pub fn token_transfer<'a>(
        swap: &Pubkey,
        token_program: AccountInfo<'a>,
        source: AccountInfo<'a>,
        destination: AccountInfo<'a>,
        authority: AccountInfo<'a>,
        nonce: u8,
        amount: u64,
    ) -> Result<(), ProgramError> {
        let swap_bytes = swap.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[nonce]];
        let signers = &[&authority_signature_seeds[..]];
        let ix = spl_token::instruction::transfer(
            token_program.key,
            source.key,
            destination.key,
            authority.key,
            &[],
            amount,
        )?;
        // invoke(&ix, &[source, destination, authority, token_program])
        invoke_signed(
            &ix,
            &[source, destination, authority, token_program],
            signers,
        )
    }
}

impl PrintProgramError for OneSolError {
    fn print<E>(&self)
    where
        E: 'static + std::error::Error + DecodeError<E> + PrintProgramError + FromPrimitive,
    {
        match self {
            OneSolError::Unknown => msg!("Error: Unknown"),
            OneSolError::ExceededSlippage => msg!("Error: ExceededSlippage"),
            OneSolError::IncorrectSwapAccount => msg!("Error: IncorrectSwapAccount"),
            OneSolError::InvalidDelegate => msg!("Error: InvalidDelegate"),
            OneSolError::InvalidCloseAuthority => msg!("Error: InvalidCloseAuthority"),
            OneSolError::InvalidInstruction => msg!("Error: InvalidInstruction"),
            OneSolError::InvalidInput => msg!("Error: InvalidInput"),
            OneSolError::InvalidOwner => msg!("Error: InvalidOwner"),
            OneSolError::InvalidProgramAddress => msg!("Error: InvildProgramAddress"),
            OneSolError::ExpectedAccount => msg!("Error: ExpectedAccount"),
            OneSolError::IncorrectTokenProgramId => msg!("Error: IncorrectTokenProgramId"),
            OneSolError::ConversionFailure => msg!("Error: ConversionFailure"),
            OneSolError::ZeroTradingTokens => msg!("Error: ZeroTradingTokens"),
            OneSolError::InternalError => msg!("Error: InternalError"),
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     // #[test]
//     // fn test_find_best_parts() {
//     //     let r = find_best_parts(10, 2);
//     //     assert_eq!(r, 8);
//     //     let r = find_best_parts(10, 8);
//     //     assert_eq!(r, 2);
//     //     let r = find_best_parts(10, 9);
//     //     assert_eq!(r, 2);
//     //     let r = find_best_parts(10, 1);
//     //     assert_eq!(r, 16);
//     // }
// }
