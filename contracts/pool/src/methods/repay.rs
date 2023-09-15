use common::FixedI128;
use debt_token_interface::DebtTokenClient;
use pool_interface::types::error::Error;
use pool_interface::types::reserve_data::ReserveData;
use soroban_sdk::{token, Address, Env};

use crate::event;
use crate::storage::{
    add_stoken_underlying_balance, read_reserve, read_token_total_supply, read_treasury,
    write_token_total_supply,
};
use crate::types::user_configurator::UserConfigurator;

use super::utils::get_collat_coeff::get_collat_coeff;
use super::utils::rate::get_actual_borrower_accrued_rate;
use super::utils::recalculate_reserve_data::recalculate_reserve_data;
use super::utils::validation::{
    require_active_reserve, require_debt, require_not_paused, require_positive_amount,
};

pub fn repay(env: &Env, who: &Address, asset: &Address, amount: i128) -> Result<(), Error> {
    who.require_auth();

    require_not_paused(env);
    require_positive_amount(env, amount);

    let reserve = read_reserve(env, asset)?;
    require_active_reserve(env, &reserve);

    let mut user_configurator = UserConfigurator::new(env, who, false);
    let user_config = user_configurator.user_config()?;
    require_debt(env, user_config, reserve.get_id());

    let s_token_supply = read_token_total_supply(env, &reserve.s_token_address);
    let debt_token_supply = read_token_total_supply(env, &reserve.debt_token_address);

    let debt_coeff = get_actual_borrower_accrued_rate(env, &reserve)?;
    let collat_coeff = get_collat_coeff(env, &reserve, s_token_supply, debt_token_supply)?;

    let (is_repayed, debt_token_supply_after) = do_repay(
        env,
        who,
        asset,
        &reserve,
        collat_coeff,
        debt_coeff,
        debt_token_supply,
        DebtTokenClient::new(env, &reserve.debt_token_address).balance(who),
        amount,
    )?;

    user_configurator
        .repay(reserve.get_id(), is_repayed)?
        .write();

    recalculate_reserve_data(
        env,
        asset,
        &reserve,
        s_token_supply,
        debt_token_supply_after,
    )?;

    Ok(())
}

/// Returns
/// bool: the flag indicating the debt is fully repayed
/// i128: total debt after repayment
#[allow(clippy::too_many_arguments)]
pub fn do_repay(
    env: &Env,
    who: &Address,
    asset: &Address,
    reserve: &ReserveData,
    collat_coeff: FixedI128,
    debt_coeff: FixedI128,
    debt_token_supply: i128,
    who_debt: i128,
    amount: i128,
) -> Result<(bool, i128), Error> {
    let borrower_actual_debt = debt_coeff
        .mul_int(who_debt)
        .ok_or(Error::MathOverflowError)?;

    let (borrower_payback_amount, borrower_debt_to_burn, is_repayed) =
        if amount >= borrower_actual_debt {
            // To avoid dust in debt_token borrower balance in case of full repayment
            (borrower_actual_debt, who_debt, true)
        } else {
            let borrower_debt_to_burn = debt_coeff
                .recip_mul_int(amount)
                .ok_or(Error::MathOverflowError)?;
            (amount, borrower_debt_to_burn, false)
        };

    let lender_part = collat_coeff
        .mul_int(borrower_debt_to_burn)
        .ok_or(Error::MathOverflowError)?;
    let treasury_part = borrower_payback_amount
        .checked_sub(lender_part)
        .ok_or(Error::MathOverflowError)?;
    let debt_token_supply_after = debt_token_supply
        .checked_sub(borrower_debt_to_burn)
        .ok_or(Error::MathOverflowError)?;

    let treasury_address = read_treasury(env);

    let underlying_asset = token::Client::new(env, asset);

    underlying_asset.transfer(who, &reserve.s_token_address, &lender_part);
    underlying_asset.transfer(who, &treasury_address, &treasury_part);
    DebtTokenClient::new(env, &reserve.debt_token_address).burn(who, &borrower_debt_to_burn);

    add_stoken_underlying_balance(env, &reserve.s_token_address, lender_part)?;
    write_token_total_supply(env, &reserve.debt_token_address, debt_token_supply_after)?;

    event::repay(env, who, asset, borrower_payback_amount);

    Ok((is_repayed, debt_token_supply_after))
}
