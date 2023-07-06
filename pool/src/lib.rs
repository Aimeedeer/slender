#![deny(warnings)]
#![no_std]
use common::{PercentageMath, RateMath, RATE_DENOMINATOR};
use pool_interface::*;
use soroban_sdk::{
    assert_with_error, contractimpl, panic_with_error, token, Address, BytesN, Env, Vec,
};

mod event;
mod storage;

use crate::storage::*;

#[allow(dead_code)] //TODO: rmeove after use borrow_
#[derive(Debug, Clone, Copy)]
struct AccountData {
    collateral: i128,
    debt: i128,
    ltv: i128,
    liquidation_threshold: i128,
    health_factor: i128,
}

//TODO: set right value for liquidation threshold
const HEALTH_FACTOR_LIQUIDATION_THRESHOLD: i128 = 1;

pub struct LendingPool;

#[contractimpl]
impl LendingPoolTrait for LendingPool {
    // Initializes the contract with the specified admin address.
    ///
    /// # Arguments
    ///
    /// - admin - The address of the admin for the contract.
    ///
    /// # Panics
    ///
    /// Panics with `AlreadyInitialized` if the admin key already exists in storage.
    ///
    fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if has_admin(&env) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }

        write_admin(&env, admin);
        Ok(())
    }

    /// Initializes a reserve for a given asset.
    ///
    /// # Arguments
    ///
    /// - asset - The address of the asset associated with the reserve.
    /// - input - The input parameters for initializing the reserve.
    ///
    /// # Panics
    ///
    /// - Panics with `Uninitialized` if the admin key is not exist in storage.
    /// - Panics if the caller is not the admin.
    /// - Panics with `ReserveAlreadyInitialized` if the specified asset key already exists in storage.
    ///
    fn init_reserve(env: Env, asset: Address, input: InitReserveInput) -> Result<(), Error> {
        Self::ensure_admin(&env)?;
        // ensure_contract(env, asset)?;
        if has_reserve(&env, asset.clone()) {
            panic_with_error!(&env, Error::ReserveAlreadyInitialized);
        }

        let mut reserve_data = ReserveData::new(&env, input);
        let mut reserves = read_reserves(&env);

        let id = reserves.len() as u8;
        reserve_data.id = BytesN::from_array(&env, &[id; 1]);
        reserves.push_back(asset.clone());

        write_reserves(&env, &reserves);
        write_reserve(&env, asset, &reserve_data);

        Ok(())
    }

    /// Retrieves the reserve data for the specified asset.
    ///
    /// # Arguments
    ///
    /// - asset - The address of the asset associated with the reserve.
    ///
    /// # Returns
    ///
    /// Returns the reserve data for the specified asset if it exists, or None otherwise.
    ///
    fn get_reserve(env: Env, asset: Address) -> Option<ReserveData> {
        read_reserve(&env, asset).ok()
    }

    /// Deposits a specified amount of an asset into the reserve associated with the asset.
    /// Depositor receives s-tokens according to the current index value.
    ///
    /// # Arguments
    ///
    /// - who - The address of the user making the deposit.
    /// - asset - The address of the asset to be deposited.
    /// - amount - The amount to be deposited.
    ///
    /// # Errors
    ///
    /// Returns `NoReserveExistForAsset` if no reserve exists for the specified asset.
    /// Returns `MathOverflowError' if an overflow occurs when calculating the amount of the s-token to be minted.
    ///
    /// # Panics
    ///
    /// If the caller is not authorized.
    /// If the deposit amount is invalid or does not meet the reserve requirements.
    /// If the reserve data cannot be retrieved from storage.
    ///
    fn deposit(env: Env, who: Address, asset: Address, amount: i128) -> Result<(), Error> {
        who.require_auth();

        let mut reserve = read_reserve(&env, asset.clone())?;
        Self::validate_deposit(&reserve, &env, amount);

        // Updates the reserve indexes and the timestamp of the update.
        // Implement later with rates.
        reserve.update_state();
        // TODO: write reserve into storage

        let is_first_deposit = Self::do_deposit(
            &env,
            &who,
            &reserve.s_token_address,
            &asset,
            amount,
            reserve.liquidity_index,
        )?;

        if is_first_deposit {
            let mut user_config: UserConfiguration =
                read_user_config(&env, who.clone()).unwrap_or_default();

            user_config.set_using_as_collateral(&env, reserve.get_id(), true);
            write_user_config(&env, who.clone(), &user_config);
            event::reserve_used_as_collateral_enabled(&env, who.clone(), asset.clone());
        }

        event::deposit(&env, who, asset, amount);

        Ok(())
    }

    fn finalize_transfer(
        _asset: Address,
        _from: Address,
        _to: Address,
        _amount: i128,
        _balance_from_before: i128,
        _balance_to_before: i128,
    ) {
        // mock to use in s_token
        // whenNotPaused
    }

    /// Withdraws a specified amount of an asset from the reserve and transfers it to the caller.
    /// Burn s-tokens from depositor according to the current index value.
    ///
    /// # Arguments
    ///
    /// - who - The address of the user making the withdrawal.
    /// - asset - The address of the asset to be withdrawn.
    /// - amount - The amount to be withdrawn. Use i128::MAX to withdraw the maximum available amount.
    /// - to - The address of the recipient of the withdrawn asset.
    ///
    /// # Errors
    ///
    /// Returns `NoReserveExistForAsset` if no reserve exists for the specified asset.
    /// Returns `UserConfigNotExists` if the user configuration does not exist in storage.
    /// Returns `MathOverflowError' if an overflow occurs when calculating the amount of the s-token to be burned.
    ///
    /// # Panics
    ///
    /// Panics if the caller is not authorized.
    /// Panics if the withdrawal amount is invalid or does not meet the reserve requirements.
    fn withdraw(
        env: Env,
        who: Address,
        asset: Address,
        amount: i128,
        to: Address,
    ) -> Result<(), Error> {
        who.require_auth();

        let mut reserve = read_reserve(&env, asset.clone())?;

        let s_token = s_token_interface::STokenClient::new(&env, &reserve.s_token_address);
        let who_balance = s_token.balance(&who);
        let amount_to_withdraw = if amount == i128::MAX {
            who_balance
        } else {
            amount
        };

        Self::validate_withdraw(&reserve, &env, amount_to_withdraw, who_balance);

        let mut user_config: UserConfiguration =
            read_user_config(&env, who.clone()).ok_or(Error::UserConfigNotExists)?;

        reserve.update_state();
        //TODO: update interest rates
        // reserve.update_interest_rates(
        //     asset.clone(),
        //     reserve.s_token_address.clone(),
        //     -amount_to_withdraw,
        // );

        //TODO: save new reserve

        if amount_to_withdraw == who_balance {
            user_config.set_using_as_collateral(&env, reserve.get_id(), false);
            write_user_config(&env, who.clone(), &user_config);
            event::reserve_used_as_collateral_disabled(&env, who.clone(), asset.clone());
        }

        let amount_to_burn = amount_to_withdraw
            .div_rate_floor(reserve.liquidity_index)
            .ok_or(Error::MathOverflowError)?;
        s_token.burn(&who, &amount_to_burn, &amount_to_withdraw, &to);

        event::withdraw(&env, who, asset, to, amount_to_withdraw);
        Ok(())
    }

    /// Allows users to borrow a specific `amount` of the reserve underlying asset, provided that the borrower
    /// already deposited enough collateral, or he was given enough allowance by a credit delegator on the
    /// corresponding debt token
    ///
    /// # Arguments
    /// - who The address of user performing borrowing
    /// - asset The address of the underlying asset to borrow
    /// - amount The amount to be borrowed
    ///
    fn borrow(env: Env, who: Address, asset: Address, amount: i128) -> Result<(), Error> {
        let mut reserve = read_reserve(&env, asset.clone())?;
        let user_config = read_user_config(&env, who.clone()).ok_or(Error::UserConfigNotExists)?;

        let amount_in_xlm = amount
            .checked_mul(Self::get_asset_price(&asset))
            .ok_or(Error::MathOverflowError)?;
        //TODO: uncomment when oracle will be implemented
        //.checked_div(10_i128.pow(reserve.configuration.decimals))
        //.ok_or(Error::MathOverflowError)?;

        Self::validate_borrow(
            &env,
            who.clone(),
            &reserve,
            &user_config,
            amount,
            amount_in_xlm,
        )?;

        let debt_token = token::Client::new(&env, &reserve.debt_token_address);
        let is_first_borrowing = debt_token.balance(&who) == 0;
        debt_token.mint(&who, &amount);
        if is_first_borrowing {
            let mut user_config = user_config;
            user_config.set_borrowing(&env, reserve.get_id(), true);
            write_user_config(&env, who.clone(), &user_config);
        }

        reserve.update_interest_rate();
        write_reserve(&env, asset.clone(), &reserve);

        let s_token = s_token_interface::STokenClient::new(&env, &reserve.s_token_address);
        s_token.transfer_underlying_to(&who, &amount);

        event::borrow(&env, who, asset, amount);

        Ok(())
    }

    #[cfg(any(test, feature = "testutils"))]
    fn set_liq_index(env: Env, asset: Address, value: i128) -> Result<(), Error> {
        let mut reserve_data = read_reserve(&env, asset.clone())?;
        reserve_data.liquidity_index = value;
        write_reserve(&env, asset, &reserve_data);

        Ok(())
    }
}

impl LendingPool {
    fn ensure_admin(env: &Env) -> Result<(), Error> {
        let admin: Address = read_admin(env)?;
        admin.require_auth();
        Ok(())
    }

    fn do_deposit(
        env: &Env,
        who: &Address,
        s_token_address: &Address,
        asset: &Address,
        amount: i128,
        liquidity_index: i128,
    ) -> Result<bool, Error> {
        let token = token::Client::new(env, asset);
        token.transfer(who, s_token_address, &amount);

        let s_token = s_token_interface::STokenClient::new(env, s_token_address);
        let is_first_deposit = s_token.balance(who) == 0;
        let amount_to_mint = amount
            .div_rate_floor(liquidity_index)
            .ok_or(Error::MathOverflowError)?;
        s_token.mint(who, &amount_to_mint);
        Ok(is_first_deposit)
    }

    fn validate_deposit(reserve: &ReserveData, env: &Env, amount: i128) {
        assert_with_error!(env, amount != 0, Error::InvalidAmount);
        let flags = reserve.configuration.get_flags();
        assert_with_error!(env, flags.is_active, Error::NoActiveReserve);
        assert_with_error!(env, !flags.is_frozen, Error::ReserveFrozen);
    }

    fn validate_withdraw(reserve: &ReserveData, env: &Env, amount: i128, balance: i128) {
        assert_with_error!(env, amount != 0, Error::InvalidAmount);
        let flags = reserve.configuration.get_flags();
        assert_with_error!(env, flags.is_active, Error::NoActiveReserve);
        assert_with_error!(env, amount <= balance, Error::NotEnoughAvailableUserBalance);

        //TODO: implement when rates exists
        //balance_decrease_allowed()
    }

    fn validate_borrow(
        env: &Env,
        who: Address,
        reserve: &ReserveData,
        user_config: &UserConfiguration,
        amount: i128,
        amount_in_xlm: i128,
    ) -> Result<(), Error> {
        assert_with_error!(env, amount != 0, Error::InvalidAmount);
        let flags = reserve.configuration.get_flags();
        assert_with_error!(env, flags.is_active, Error::NoActiveReserve);
        assert_with_error!(env, !flags.is_frozen, Error::ReserveFrozen);
        assert_with_error!(env, !flags.borrowing_enabled, Error::BorrowingNotEnabled);

        let reserves = &read_reserves(env);
        let account_data = Self::calc_account_data(env, who.clone(), user_config, reserves)?;

        assert_with_error!(env, account_data.collateral > 0, Error::CollateralIsZero);
        assert_with_error!(
            env,
            account_data.health_factor > HEALTH_FACTOR_LIQUIDATION_THRESHOLD,
            Error::HealthFactorLowerThanLiqThreshold
        );

        let amount_of_collateral_needed_xlm = account_data
            .debt
            .checked_add(amount_in_xlm)
            .ok_or(Error::MathOverflowError)?
            .percent_div(account_data.ltv)
            .ok_or(Error::MathOverflowError)?;

        assert_with_error!(
            env,
            amount_of_collateral_needed_xlm <= account_data.collateral,
            Error::CollateralNotCoverNewBorrow
        );

        let coll_same_as_borrow_check = !user_config.is_using_as_collateral(env, reserve.get_id())
            || reserve.configuration.ltv == 0;

        if !coll_same_as_borrow_check {
            let s_token = s_token_interface::STokenClient::new(env, &reserve.s_token_address);
            let coll_coeff = Self::get_collateral_coeff(env, &reserve)?;
            let compounded_balance = s_token
                .balance(&who)
                .mul_rate_floor(coll_coeff)
                .ok_or(Error::MathOverflowError)?;

            assert_with_error!(
                env,
                amount > compounded_balance,
                Error::CollateralSameAsBorrow
            );
        }
        //TODO: check validation
        Ok(())
    }

    fn calc_account_data(
        env: &Env,
        who: Address,
        user_config: &UserConfiguration,
        reserves: &Vec<Address>,
    ) -> Result<AccountData, Error> {
        if user_config.is_empty() {
            return Err(Error::HealthFactorLowerThanLiqThreshold);
        }

        let mut total_collateral_in_xlm: i128 = 0;
        let mut total_debt_in_xlm: i128 = 0;
        let mut avg_ltv: i128 = 0;
        let mut avg_liquidation_threshold: i128 = 0;
        let reserves_count = reserves.len() as u8; //TODO: add check to init_reserve() method
                                                   // calc collateral and debt expressed in XLM token
        for i in 0..reserves_count {
            if !user_config.is_using_as_collateral_or_borrowing(env, i) {
                continue;
            }

            //TODO: avoid unwrap
            let curr_reserve_asset = reserves.get(i.into()).unwrap().unwrap();
            let curr_reserve = read_reserve(env, curr_reserve_asset.clone())?;

            //TODO: uncomment when `get_asset_price` will be implemented
            let asset_unit = 1i128; //10i128.pow(curr_reserve.configuration.decimals);
            let reserve_price = Self::get_asset_price(&curr_reserve_asset);

            if curr_reserve.configuration.liq_threshold != 0
                && user_config.is_using_as_collateral(env, i)
            {
                let coll_coeff = Self::get_collateral_coeff(env, &curr_reserve)?;

                // compounded balance of sToken
                let s_token =
                    s_token_interface::STokenClient::new(env, &curr_reserve.s_token_address);

                let compounded_balance = s_token
                    .balance(&who)
                    .mul_rate_floor(coll_coeff)
                    .ok_or(Error::MathOverflowError)?;

                let liquidity_balance_in_xlm = compounded_balance
                    .checked_mul(reserve_price)
                    .ok_or(Error::MathOverflowError)?
                    .checked_div(asset_unit)
                    .ok_or(Error::MathOverflowError)?;

                total_collateral_in_xlm = total_collateral_in_xlm
                    .checked_add(liquidity_balance_in_xlm)
                    .ok_or(Error::MathOverflowError)?;

                avg_ltv += i128::from(curr_reserve.configuration.ltv) * liquidity_balance_in_xlm;
                avg_liquidation_threshold +=
                    i128::from(curr_reserve.configuration.liq_threshold) * liquidity_balance_in_xlm;
            }

            if user_config.is_borrowing(env, i) {
                let debt_coeff = Self::get_debt_coeff(env, &curr_reserve)?;

                let debt_token = token::Client::new(env, &curr_reserve.debt_token_address);
                let compounded_balance = debt_token
                    .balance(&who)
                    .mul_rate_floor(debt_coeff)
                    .ok_or(Error::MathOverflowError)?;

                let debt_balance_in_xlm = compounded_balance
                    .checked_mul(reserve_price)
                    .ok_or(Error::MathOverflowError)?
                    .checked_div(asset_unit)
                    .ok_or(Error::MathOverflowError)?;

                total_debt_in_xlm = total_debt_in_xlm
                    .checked_add(debt_balance_in_xlm)
                    .ok_or(Error::MathOverflowError)?;
            }
        }

        avg_ltv = avg_ltv.checked_div(total_collateral_in_xlm).unwrap_or(0);
        avg_liquidation_threshold = avg_liquidation_threshold
            .checked_div(total_collateral_in_xlm)
            .unwrap_or(0);

        Ok(AccountData {
            collateral: total_collateral_in_xlm,
            debt: total_debt_in_xlm,
            ltv: avg_ltv,
            liquidation_threshold: avg_liquidation_threshold,
            health_factor: Self::calc_health_factor(
                total_collateral_in_xlm,
                total_debt_in_xlm,
                avg_liquidation_threshold,
            )?,
        })
    }

    /// Price of asset expressed in XLM token
    fn get_asset_price(_asset: &Address) -> i128 {
        1i128
    }

    fn calc_health_factor(
        total_collateral: i128,
        total_debt: i128,
        liquidation_threshold: i128,
    ) -> Result<i128, Error> {
        if total_debt == 0 {
            return Ok(-1i128);
        }

        total_collateral
            .percent_mul(liquidation_threshold)
            .ok_or(Error::MathOverflowError)?
            .checked_div(total_debt)
            .ok_or(Error::MathOverflowError)
        // return (totalCollateralInETH.percentMul(liquidationThreshold)).wadDiv(totalDebtInETH);
    }

    fn get_collateral_coeff(_env: &Env, _reserve: &ReserveData) -> Result<i128, Error> {
        //TODO: implement rate
        Ok(RATE_DENOMINATOR)

        // let last_update_timestamp = reserve.last_update_timestamp;
        // let current_timestamp = env.ledger().timestamp();

        // if last_update_timestamp == current_timestamp {
        //     return Ok(reserve.liquidity_index);
        // }

        // let time_delta = current_timestamp
        //     .checked_sub(last_update_timestamp)
        //     .ok_or(Error::MathOverflowError)?;

        // MathUtils::calc_linear_interest(reserve.liquidity_index, time_delta)
        //     .ok_or(Error::MathOverflowError)
    }

    fn get_debt_coeff(_env: &Env, _reserve: &ReserveData) -> Result<i128, Error> {
        //TODO: implement accrued
        Ok(RATE_DENOMINATOR)
    }
}

#[cfg(test)]
mod test;
