use crate::rate::{calc_interest_rate, calc_next_accrued_rate};
use crate::tests::sut::{
    create_pool_contract, create_price_feed_contract, create_s_token_contract,
    create_token_contract, init_pool, DAY,
};
use crate::*;
use common::FixedI128;
use debt_token_interface::DebtTokenClient;
use price_feed_interface::PriceFeedClient;
use s_token_interface::STokenClient;
use soroban_sdk::testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke};
use soroban_sdk::{vec, IntoVal};

use super::sut::fill_pool;

extern crate std;

#[test]
fn init_reserve() {
    let env = Env::default();

    let admin = Address::random(&env);
    let token_admin = Address::random(&env);

    let (underlying_token, _) = create_token_contract(&env, &token_admin);
    let (debt_token, _) = create_token_contract(&env, &token_admin);

    let pool: LendingPoolClient<'_> = create_pool_contract(&env, &admin);
    let s_token = create_s_token_contract(&env, &pool.address, &underlying_token.address);
    assert!(pool.get_reserve(&underlying_token.address).is_none());

    let init_reserve_input = InitReserveInput {
        s_token_address: s_token.address.clone(),
        debt_token_address: debt_token.address.clone(),
    };

    assert_eq!(
        pool.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &pool.address,
                fn_name: "init_reserve",
                args: (&underlying_token.address, init_reserve_input.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .init_reserve(&underlying_token.address, &init_reserve_input),
        ()
    );

    let reserve = pool.get_reserve(&underlying_token.address).unwrap();

    assert!(pool.get_reserve(&underlying_token.address).is_some());
    assert_eq!(init_reserve_input.s_token_address, reserve.s_token_address);
    assert_eq!(
        init_reserve_input.debt_token_address,
        reserve.debt_token_address
    );
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn init_reserve_second_time() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    let init_reserve_input = InitReserveInput {
        s_token_address: sut.s_token().address.clone(),
        debt_token_address: sut.debt_token().address.clone(),
    };

    //TODO: check error after soroban fix
    sut.pool
        .init_reserve(&sut.token().address, &init_reserve_input);

    // assert_eq!(
    //     sut.pool
    //         .try_init_reserve(&sut.token().address, &init_reserve_input)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::ReserveAlreadyInitialized
    // )
}

#[test]
fn init_reserve_when_pool_not_initialized() {
    let env = Env::default();

    let admin = Address::random(&env);
    let token_admin = Address::random(&env);

    let (underlying_token, _) = create_token_contract(&env, &token_admin);
    let (debt_token, _) = create_token_contract(&env, &token_admin);

    let pool: LendingPoolClient<'_> =
        LendingPoolClient::new(&env, &env.register_contract(None, LendingPool));
    let s_token = create_s_token_contract(&env, &pool.address, &underlying_token.address);
    assert!(pool.get_reserve(&underlying_token.address).is_none());

    let init_reserve_input = InitReserveInput {
        s_token_address: s_token.address.clone(),
        debt_token_address: debt_token.address.clone(),
    };

    assert_eq!(
        pool.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &pool.address,
                fn_name: "init_reserve",
                args: (&underlying_token.address, init_reserve_input.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .try_init_reserve(&underlying_token.address, &init_reserve_input)
        .unwrap_err()
        .unwrap(),
        Error::Uninitialized
    );
}

#[test]
fn set_ir_params() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    let ir_params_input = IRParams {
        alpha: 144,
        initial_rate: 201,
        max_rate: 50_001,
        scaling_coeff: 9_001,
    };

    sut.pool.set_ir_params(&ir_params_input);

    let ir_params = sut.pool.ir_params().unwrap();

    assert_eq!(ir_params_input.alpha, ir_params.alpha);
    assert_eq!(ir_params_input.initial_rate, ir_params.initial_rate);
    assert_eq!(ir_params_input.max_rate, ir_params.max_rate);
    assert_eq!(ir_params_input.scaling_coeff, ir_params.scaling_coeff);
}

#[test]
fn borrow() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    let initial_amount: i128 = 1_000_000_000;
    let lender = Address::random(&env);
    let borrower = Address::random(&env);

    for r in sut.reserves.iter() {
        r.token_admin.mint(&lender, &initial_amount);
        assert_eq!(r.token.balance(&lender), initial_amount);

        r.token_admin.mint(&borrower, &initial_amount);
        assert_eq!(r.token.balance(&borrower), initial_amount);
    }

    //TODO: optimize gas
    env.budget().reset_unlimited();

    //lender deposit all tokens
    let deposit_amount = 100_000_000;
    for r in sut.reserves.iter() {
        let pool_balance = r.token.balance(&r.s_token.address);
        sut.pool.deposit(&lender, &r.token.address, &deposit_amount);
        assert_eq!(r.s_token.balance(&lender), deposit_amount);
        assert_eq!(
            r.token.balance(&r.s_token.address),
            pool_balance + deposit_amount
        );
    }

    //borrower deposit first token and borrow second token
    sut.pool
        .deposit(&borrower, &sut.reserves[0].token.address, &deposit_amount);
    assert_eq!(sut.reserves[0].s_token.balance(&borrower), deposit_amount);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    assert_eq!(s_token_underlying_supply, 100_000_000);

    //borrower borrow second token
    let borrow_asset = sut.reserves[1].token.address.clone();
    let borrow_amount = 10_000;
    let pool_balance_before = sut.reserves[1]
        .token
        .balance(&sut.reserves[1].s_token.address);

    let borrower_balance_before = sut.reserves[1].token.balance(&borrower);
    sut.pool.borrow(&borrower, &borrow_asset, &borrow_amount);
    assert_eq!(
        sut.reserves[1].token.balance(&borrower),
        borrower_balance_before + borrow_amount
    );

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let pool_balance = sut.reserves[1]
        .token
        .balance(&sut.reserves[1].s_token.address);
    let debt_token_balance = sut.reserves[1].debt_token.balance(&borrower);
    assert_eq!(
        pool_balance + borrow_amount,
        pool_balance_before,
        "Pool balance"
    );
    assert_eq!(debt_token_balance, borrow_amount, "Debt token balance");
    assert_eq!(s_token_underlying_supply, 99_990_000);
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn borrow_utilization_exceeded() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    let initial_amount: i128 = 1_000_000_000;
    let lender = Address::random(&env);
    let borrower = Address::random(&env);

    sut.reserves[0].token_admin.mint(&lender, &initial_amount);
    sut.reserves[1].token_admin.mint(&borrower, &initial_amount);

    //TODO: optimize gas
    env.budget().reset_unlimited();

    let deposit_amount = 1_000_000_000;

    sut.pool
        .deposit(&lender, &sut.reserves[0].token.address, &deposit_amount);

    sut.pool
        .deposit(&borrower, &sut.reserves[1].token.address, &deposit_amount);

    sut.pool
        .borrow(&borrower, &sut.reserves[0].token.address, &990_000_000);

    // assert_eq!(
    //     sut.pool
    //         .try_borrow(&borrower, &sut.reserves[0].token.address, &990_000_000)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::UtilizationCapExceeded
    // )
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn borrow_user_confgig_not_exists() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);
    let borrower = Address::random(&env);

    //TODO: check error after soroban fix
    let borrow_amount = 0;
    sut.pool
        .borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount);
    // assert_eq!(
    //     sut.pool
    //         .try_borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::UserConfigNotExists
    // )
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn borrow_collateral_is_zero() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);
    let lender = Address::random(&env);
    let borrower = Address::random(&env);

    let initial_amount = 1_000_000_000;
    for r in sut.reserves.iter() {
        r.token_admin.mint(&borrower, &initial_amount);
        assert_eq!(r.token.balance(&borrower), initial_amount);
        r.token_admin.mint(&lender, &initial_amount);
        assert_eq!(r.token.balance(&lender), initial_amount);
    }

    let deposit_amount = 1000;

    env.budget().reset_unlimited();

    sut.pool
        .deposit(&lender, &sut.reserves[0].token.address, &deposit_amount);

    sut.pool
        .deposit(&borrower, &sut.reserves[1].token.address, &deposit_amount);

    sut.pool.withdraw(
        &borrower,
        &sut.reserves[1].token.address,
        &deposit_amount,
        &borrower,
    );

    let borrow_amount = 100;
    sut.pool
        .borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount)

    //TODO: check error after fix
    // assert_eq!(
    //     sut.pool
    //         .try_borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::CollateralNotCoverNewBorrow
    // )
}

#[test]
fn borrow_no_active_reserve() {
    //TODO: implement
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn borrow_collateral_not_cover_new_debt() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);
    let lender = Address::random(&env);
    let borrower = Address::random(&env);

    let initial_amount = 1_000_000_000;
    for r in sut.reserves.iter() {
        r.token_admin.mint(&borrower, &initial_amount);
        assert_eq!(r.token.balance(&borrower), initial_amount);
        r.token_admin.mint(&lender, &initial_amount);
        assert_eq!(r.token.balance(&lender), initial_amount);
    }

    let borrower_deposit_amount = 500;
    let lender_deposit_amount = 2000;

    //TODO: optimize gas
    env.budget().reset_unlimited();

    sut.pool.deposit(
        &lender,
        &sut.reserves[0].token.address,
        &lender_deposit_amount,
    );

    sut.pool.deposit(
        &borrower,
        &sut.reserves[1].token.address,
        &borrower_deposit_amount,
    );

    //TODO: check error after soroban fix
    let borrow_amount = 1000;
    sut.pool
        .borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount);

    // assert_eq!(
    //     sut.pool
    //         .try_borrow(&borrower, &sut.reserves[0].token.address, &borrow_amount)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::CollateralNotCoverNewBorrow
    // )
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn borrow_disabled_for_borrowing_asset() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    let initial_amount: i128 = 1_000_000_000;
    let lender = Address::random(&env);
    let borrower = Address::random(&env);

    for r in sut.reserves.iter() {
        r.token_admin.mint(&lender, &initial_amount);
        assert_eq!(r.token.balance(&lender), initial_amount);

        r.token_admin.mint(&borrower, &initial_amount);
        assert_eq!(r.token.balance(&borrower), initial_amount);
    }

    env.budget().reset_unlimited();

    //lender deposit all tokens
    let deposit_amount = 100_000_000;
    for r in sut.reserves.iter() {
        let pool_balance = r.token.balance(&r.s_token.address);
        sut.pool.deposit(&lender, &r.token.address, &deposit_amount);
        assert_eq!(r.s_token.balance(&lender), deposit_amount);
        assert_eq!(
            r.token.balance(&r.s_token.address),
            pool_balance + deposit_amount
        );
    }

    //borrower deposit first token and borrow second token
    sut.pool
        .deposit(&borrower, &sut.reserves[0].token.address, &deposit_amount);
    assert_eq!(sut.reserves[0].s_token.balance(&borrower), deposit_amount);

    //borrower borrow second token
    let borrow_asset = sut.reserves[1].token.address.clone();
    let borrow_amount = 10_000;

    //disable second token for borrowing
    sut.pool.enable_borrowing_on_reserve(&borrow_asset, &false);
    let reserve = sut.pool.get_reserve(&borrow_asset);
    assert_eq!(reserve.unwrap().configuration.borrowing_enabled, false);

    //TODO: check error after soroban fix
    sut.pool.borrow(&borrower, &borrow_asset, &borrow_amount);

    // assert_eq!(
    //     sut.pool
    //         .try_borrow(&borrower, &borrow_asset, &borrow_amount)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::BorrowingNotEnabled
    // );
}

#[test]
fn set_price_feed() {
    let env = Env::default();

    let admin = Address::random(&env);
    let asset_1 = Address::random(&env);
    let asset_2 = Address::random(&env);

    let pool: LendingPoolClient<'_> = create_pool_contract(&env, &admin);
    let price_feed: PriceFeedClient<'_> = create_price_feed_contract(&env);
    let assets = vec![&env, asset_1.clone(), asset_2.clone()];

    assert!(pool.price_feed(&asset_1.clone()).is_none());
    assert!(pool.price_feed(&asset_2.clone()).is_none());

    assert_eq!(
        pool.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &pool.address,
                fn_name: "set_price_feed",
                args: (&price_feed.address, assets.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }])
        .set_price_feed(&price_feed.address, &assets.clone()),
        ()
    );

    assert_eq!(pool.price_feed(&asset_1).unwrap(), price_feed.address);
    assert_eq!(pool.price_feed(&asset_2).unwrap(), price_feed.address);
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn test_liquidate_error_good_position() {
    let env = Env::default();
    env.mock_all_auths();
    let sut = init_pool(&env);
    let liquidator = Address::random(&env);
    let user = Address::random(&env);
    let token = &sut.reserves[0].token;
    let token_admin = &sut.reserves[0].token_admin;
    token_admin.mint(&user, &1_000_000_000);

    env.budget().reset_unlimited();

    sut.pool.deposit(&user, &token.address, &1_000_000_000);

    let position = sut.pool.account_position(&user);
    assert!(position.npv > 0, "test configuration");

    //TODO: check error after soroban fix
    sut.pool.liquidate(&liquidator, &user, &false);

    // assert_eq!(
    //     sut.pool
    //         .try_liquidate(&liquidator, &user, &false)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::GoodPosition
    // );
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn test_liquidate_error_not_enough_collateral() {
    let env = Env::default();
    env.mock_all_auths();
    let sut = init_pool(&env);

    //TODO: optimize gas
    env.budget().reset_unlimited();

    let liquidator = Address::random(&env);
    let borrower = Address::random(&env);
    let lender = Address::random(&env);
    let token1 = &sut.reserves[0].token;
    let token1_admin = &sut.reserves[0].token_admin;
    let token2 = &sut.reserves[1].token;
    let token2_admin = &sut.reserves[1].token_admin;

    let deposit = 1_000_000_000;
    let discount = sut
        .pool
        .get_reserve(&token1.address)
        .expect("reserve")
        .configuration
        .discount;
    let debt = FixedI128::from_percentage(discount)
        .unwrap()
        .mul_int(deposit)
        .unwrap();
    token1_admin.mint(&borrower, &deposit);
    token2_admin.mint(&lender, &deposit);
    sut.pool.deposit(&borrower, &token1.address, &deposit);
    sut.pool.deposit(&lender, &token2.address, &deposit);
    sut.pool.borrow(&borrower, &token2.address, &debt);
    sut.price_feed.set_price(
        &token2.address,
        &(10i128.pow(sut.price_feed.decimals()) * 2),
    );

    let position = sut.pool.account_position(&borrower);
    assert!(position.npv < 0, "test configuration");

    //TODO: check error after soroban fix
    sut.pool.liquidate(&liquidator, &borrower, &false);

    // assert_eq!(
    //     sut.pool
    //         .try_liquidate(&liquidator, &borrower, &false)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::NotEnoughCollateral
    // );
}

#[test]
fn test_liquidate() {
    let env = Env::default();
    env.mock_all_auths();
    let sut = init_pool(&env);

    //TODO: optimize gas
    env.budget().reset_unlimited();

    let liquidator = Address::random(&env);
    let borrower = Address::random(&env);
    let lender = Address::random(&env);
    let collateral_asset = &sut.reserves[0].token;
    let collateral_asset_admin = &sut.reserves[0].token_admin;
    let debt_asset = &sut.reserves[1].token;
    let debt_asset_admin = &sut.reserves[1].token_admin;
    let deposit = 1_000_000_000;
    let discount = sut
        .pool
        .get_reserve(&collateral_asset.address)
        .expect("Reserve")
        .configuration
        .discount;
    let debt = FixedI128::from_percentage(discount)
        .unwrap()
        .mul_int(deposit)
        .unwrap();
    collateral_asset_admin.mint(&borrower, &deposit);
    debt_asset_admin.mint(&lender, &deposit);
    debt_asset_admin.mint(&liquidator, &deposit);

    sut.pool
        .deposit(&borrower, &collateral_asset.address, &deposit);
    sut.pool.deposit(&lender, &debt_asset.address, &deposit);

    sut.pool.borrow(&borrower, &debt_asset.address, &debt);

    let s_token_underlying_supply_0 = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[0].s_token.address);
    let s_token_underlying_supply_1 = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let position = sut.pool.account_position(&borrower);
    assert!(position.npv == 0, "test configuration");

    let debt_reserve = sut.pool.get_reserve(&debt_asset.address).expect("reserve");
    let debt_token = DebtTokenClient::new(&env, &debt_reserve.debt_token_address);
    let debt_token_supply_before = debt_token.total_supply();
    let borrower_collateral_balance_before = collateral_asset.balance(&borrower);
    let stoken = STokenClient::new(
        &env,
        &sut.pool
            .get_reserve(&collateral_asset.address)
            .expect("reserve")
            .s_token_address,
    );
    let stoken_balance_before = stoken.balance(&borrower);
    assert_eq!(s_token_underlying_supply_0, 1_000_000_000);
    assert_eq!(s_token_underlying_supply_1, 400_000_000);

    assert_eq!(sut.pool.liquidate(&liquidator, &borrower, &false), ());

    let s_token_underlying_supply_0 = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[0].s_token.address);
    let s_token_underlying_supply_1 = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let debt_with_penalty = FixedI128::from_percentage(debt_reserve.configuration.liq_bonus)
        .unwrap()
        .mul_int(debt)
        .unwrap();
    // assume that default price is 1.0 for both assets
    assert_eq!(collateral_asset.balance(&liquidator), debt_with_penalty);
    assert_eq!(debt_asset.balance(&liquidator), deposit - debt);
    assert_eq!(debt_asset.balance(&borrower), debt);
    assert_eq!(debt_token.balance(&borrower), 0);
    assert_eq!(debt_token.total_supply(), debt_token_supply_before - debt);
    assert_eq!(
        collateral_asset.balance(&borrower),
        borrower_collateral_balance_before
    );
    assert_eq!(
        stoken.balance(&borrower),
        stoken_balance_before - debt_with_penalty
    );
    assert_eq!(s_token_underlying_supply_0, 340_000_000);
    assert_eq!(s_token_underlying_supply_1, 1_000_000_000);
}

#[test]
fn test_liquidate_receive_stoken() {
    let env = Env::default();
    env.mock_all_auths();
    let sut = init_pool(&env);
    //TODO: optimize gas
    env.budget().reset_unlimited();

    let liquidator = Address::random(&env);
    let borrower = Address::random(&env);
    let lender = Address::random(&env);
    let collateral_asset = &sut.reserves[0].token;
    let collateral_asset_admin = &sut.reserves[0].token_admin;
    let debt_asset = &sut.reserves[1].token;
    let debt_asset_admin = &sut.reserves[1].token_admin;
    let deposit = 1_000_000_000;
    let discount = sut
        .pool
        .get_reserve(&collateral_asset.address)
        .expect("Reserve")
        .configuration
        .discount;
    let debt = FixedI128::from_percentage(discount)
        .unwrap()
        .mul_int(deposit)
        .unwrap();
    collateral_asset_admin.mint(&borrower, &deposit);
    debt_asset_admin.mint(&lender, &deposit);
    debt_asset_admin.mint(&liquidator, &deposit);

    sut.pool
        .deposit(&borrower, &collateral_asset.address, &deposit);
    sut.pool.deposit(&lender, &debt_asset.address, &deposit);

    sut.pool.borrow(&borrower, &debt_asset.address, &debt);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let position = sut.pool.account_position(&borrower);
    assert!(position.npv == 0, "test configuration");

    let debt_reserve = sut.pool.get_reserve(&debt_asset.address).expect("reserve");
    let debt_token = DebtTokenClient::new(&env, &debt_reserve.debt_token_address);
    let debt_token_supply_before = debt_token.total_supply();
    let borrower_collateral_balance_before = collateral_asset.balance(&borrower);
    let liquidator_collateral_balance_before = collateral_asset.balance(&liquidator);
    let stoken = STokenClient::new(
        &env,
        &sut.pool
            .get_reserve(&collateral_asset.address)
            .expect("reserve")
            .s_token_address,
    );
    let borrower_stoken_balance_before = stoken.balance(&borrower);
    let liquidator_stoken_balance_before = stoken.balance(&liquidator);

    assert_eq!(s_token_underlying_supply, 400_000_000);

    assert_eq!(sut.pool.liquidate(&liquidator, &borrower, &true), ());

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let debt_with_penalty = FixedI128::from_percentage(debt_reserve.configuration.liq_bonus)
        .unwrap()
        .mul_int(debt)
        .unwrap();
    // assume that default price is 1.0 for both assets
    assert_eq!(
        collateral_asset.balance(&liquidator),
        liquidator_collateral_balance_before
    );
    assert_eq!(debt_asset.balance(&liquidator), deposit - debt);
    assert_eq!(debt_asset.balance(&borrower), debt);
    assert_eq!(debt_token.balance(&borrower), 0);
    assert_eq!(debt_token.total_supply(), debt_token_supply_before - debt);
    assert_eq!(
        collateral_asset.balance(&borrower),
        borrower_collateral_balance_before
    );
    assert_eq!(
        stoken.balance(&borrower),
        borrower_stoken_balance_before - debt_with_penalty
    );
    assert_eq!(
        stoken.balance(&liquidator),
        liquidator_stoken_balance_before + debt_with_penalty
    );
    assert_eq!(s_token_underlying_supply, 1_000_000_000);
}

#[test]
fn liquidate_over_repay_liquidator_debt() {
    let env = Env::default();
    env.mock_all_auths();
    let sut = init_pool(&env);

    env.budget().reset_unlimited();

    let liquidator = Address::random(&env);
    let borrower = Address::random(&env);
    let lender = Address::random(&env);

    let reserve_1 = &sut.reserves[0];
    let reserve_2 = &sut.reserves[1];

    reserve_1.token_admin.mint(&liquidator, &2_000_000_000);
    reserve_1.token_admin.mint(&borrower, &2_000_000_000);
    reserve_2.token_admin.mint(&lender, &2_000_000_000);
    reserve_2.token_admin.mint(&liquidator, &2_000_000_000);

    sut.pool
        .deposit(&lender, &reserve_2.token.address, &2_000_000_000);
    sut.pool
        .deposit(&liquidator, &reserve_2.token.address, &1_000_000_000);
    sut.pool
        .deposit(&borrower, &reserve_1.token.address, &1_000_000_000);

    let s_token_underlying_supply_1 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_1.s_token.address);
    let s_token_underlying_supply_2 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_2.s_token.address);

    assert_eq!(s_token_underlying_supply_1, 1_000_000_000);
    assert_eq!(s_token_underlying_supply_2, 3_000_000_000);

    sut.pool
        .borrow(&borrower, &reserve_2.token.address, &600_000_000);
    sut.pool
        .borrow(&liquidator, &reserve_1.token.address, &200_000_000);

    let s_token_underlying_supply_1 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_1.s_token.address);
    let s_token_underlying_supply_2 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_2.s_token.address);

    assert_eq!(s_token_underlying_supply_1, 800_000_000);
    assert_eq!(s_token_underlying_supply_2, 2_400_000_000);

    let borrower_debt_before = reserve_2.debt_token.balance(&borrower);
    let liquidator_debt_before = reserve_1.debt_token.balance(&liquidator);

    let borrower_collat_before = reserve_1.s_token.balance(&borrower);
    let liquidator_collat_before = reserve_2.s_token.balance(&liquidator);

    assert_eq!(sut.pool.liquidate(&liquidator, &borrower, &true), ());

    let s_token_underlying_supply_1 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_1.s_token.address);
    let s_token_underlying_supply_2 = sut
        .pool
        .get_stoken_underlying_balance(&reserve_2.s_token.address);

    let borrower_debt_after = reserve_2.debt_token.balance(&borrower);
    let liquidator_debt_after = reserve_1.debt_token.balance(&liquidator);

    let borrower_collat_after = reserve_1.s_token.balance(&borrower);
    let liquidator_collat_after = reserve_1.s_token.balance(&liquidator);

    // borrower borrowed 600_000_000
    assert_eq!(borrower_debt_before, 600_000_000);

    // liquidator borrowed 200_000_000
    assert_eq!(liquidator_debt_before, 200_000_000);

    // borrower deposited 1_000_000_000
    assert_eq!(borrower_collat_before, 1_000_000_000);

    // liquidator deposited 1_000_000_000
    assert_eq!(liquidator_collat_before, 1_000_000_000);

    // borrower's debt repayed
    assert_eq!(borrower_debt_after, 0);

    // liquidator's debt repayed
    assert_eq!(liquidator_debt_after, 0);

    // borrower transferred stokens: 1_000_000_000 - 660_000_000 = 340_000_000
    assert_eq!(borrower_collat_after, 340_000_000);

    // liquidator accepted stokens: 660_000_000 - 200_000_000 = 460_000_000
    assert_eq!(liquidator_collat_after, 460_000_000);

    assert_eq!(s_token_underlying_supply_1, 800_000_000);
    assert_eq!(s_token_underlying_supply_2, 3_000_000_000);
}

#[test]
fn user_operation_should_update_ar_coeffs() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    //TODO: optimize gas
    env.budget().reset_unlimited();

    let debt_asset_1 = sut.reserves[1].token.address.clone();

    let lender = Address::random(&env);
    let borrower_1 = Address::random(&env);
    let borrow_amount = 40_000_000;

    //init pool with one borrower and one lender
    let initial_amount: i128 = 1_000_000_000;
    for r in sut.reserves.iter() {
        r.token_admin.mint(&lender, &initial_amount);
        r.token_admin.mint(&borrower_1, &initial_amount);
    }

    //lender deposit all tokens
    let deposit_amount = 100_000_000;
    for r in sut.reserves.iter() {
        sut.pool.deposit(&lender, &r.token.address, &deposit_amount);
    }

    sut.pool
        .deposit(&borrower_1, &sut.reserves[0].token.address, &deposit_amount);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    assert_eq!(s_token_underlying_supply, 100_000_000);

    // ensure that zero elapsed time doesn't change AR coefficients
    {
        let reserve_before = sut.pool.get_reserve(&debt_asset_1).unwrap();
        sut.pool.borrow(&borrower_1, &debt_asset_1, &borrow_amount);

        let s_token_underlying_supply = sut
            .pool
            .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

        let updated_reserve = sut.pool.get_reserve(&debt_asset_1).unwrap();
        assert_eq!(
            updated_reserve.lender_accrued_rate,
            reserve_before.lender_accrued_rate
        );
        assert_eq!(
            updated_reserve.borrower_accrued_rate,
            reserve_before.borrower_accrued_rate
        );
        assert_eq!(
            reserve_before.last_update_timestamp,
            updated_reserve.last_update_timestamp
        );
        assert_eq!(s_token_underlying_supply, 60_000_000);
    }

    // shift time to
    env.ledger().with_mut(|li| {
        li.timestamp = 24 * 60 * 60 // one day
    });

    //second deposit by lender of debt asset
    sut.pool.deposit(&lender, &debt_asset_1, &deposit_amount);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[1].s_token.address);

    let updated = sut.pool.get_reserve(&debt_asset_1).unwrap();
    let ir_params = sut.pool.ir_params().unwrap();
    let debt_ir = calc_interest_rate(deposit_amount, borrow_amount, &ir_params).unwrap();
    let lender_ir = debt_ir
        .checked_mul(FixedI128::from_percentage(ir_params.scaling_coeff).unwrap())
        .unwrap();

    let elapsed_time = env.ledger().timestamp();

    let coll_ar = calc_next_accrued_rate(FixedI128::ONE, lender_ir, elapsed_time)
        .unwrap()
        .into_inner();
    let debt_ar = calc_next_accrued_rate(FixedI128::ONE, debt_ir, elapsed_time)
        .unwrap()
        .into_inner();

    assert_eq!(updated.lender_accrued_rate, coll_ar);
    assert_eq!(updated.borrower_accrued_rate, debt_ar);
    assert_eq!(updated.lender_ir, lender_ir.into_inner());
    assert_eq!(updated.borrower_ir, debt_ir.into_inner());
    assert_eq!(s_token_underlying_supply, 160_000_000);
}

#[test]
fn borrow_should_mint_debt_token() {
    let env = Env::default();
    env.mock_all_auths();

    //TODO: optimize gas

    let sut = init_pool(&env);

    env.budget().reset_unlimited();

    let (_lender, borrower, debt_config) = fill_pool(&env, &sut);
    let debt_token = &debt_config.token.address;

    // shift time to one day
    env.ledger().with_mut(|li| {
        li.timestamp = 24 * 60 * 60 // one day
    });

    let debttoken_supply = debt_config.debt_token.total_supply();
    let borrower_debt_token_balance_before = debt_config.debt_token.balance(&borrower);
    let borrow_amount = 10_000;
    sut.pool.borrow(&borrower, &debt_token, &borrow_amount);

    let reserve = sut.pool.get_reserve(&debt_token).unwrap();
    let expected_minted_debt_token = FixedI128::from_inner(reserve.borrower_accrued_rate)
        .recip_mul_int(borrow_amount)
        .unwrap();

    assert_eq!(
        debt_config.debt_token.balance(&borrower),
        borrower_debt_token_balance_before + expected_minted_debt_token
    );
    assert_eq!(
        debt_config.debt_token.balance(&borrower),
        debttoken_supply + expected_minted_debt_token
    )
}

#[test]
fn collateral_coeff_test() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    env.budget().reset_unlimited();

    let (_lender, borrower, debt_config) = fill_pool(&env, &sut);
    let initial_collat_coeff = sut.pool.collat_coeff(&debt_config.token.address);
    std::println!("initial_collat_coeff={}", initial_collat_coeff);

    env.ledger().with_mut(|l| {
        l.timestamp = 2 * DAY;
    });

    let borrow_amount = 50_000;
    sut.pool
        .borrow(&borrower, &debt_config.token.address, &borrow_amount);
    let reserve = sut.pool.get_reserve(&debt_config.token.address).unwrap();

    let collat_ar = FixedI128::from_inner(reserve.lender_accrued_rate);
    let s_token_supply = debt_config.s_token.total_supply();
    let balance = debt_config.token.balance(&debt_config.s_token.address);
    let debt_token_suply = debt_config.debt_token.total_supply();

    let expected_collat_coeff = FixedI128::from_rational(
        balance + collat_ar.mul_int(debt_token_suply).unwrap(),
        s_token_supply,
    )
    .unwrap()
    .into_inner();

    let collat_coeff = sut.pool.collat_coeff(&debt_config.token.address);
    assert_eq!(collat_coeff, expected_collat_coeff);

    // shift time to 8 days
    env.ledger().with_mut(|l| {
        l.timestamp = 10 * DAY;
    });

    let elapsed_time = 8 * DAY;
    let collat_ar = calc_next_accrued_rate(
        collat_ar,
        FixedI128::from_inner(reserve.lender_ir),
        elapsed_time,
    )
    .unwrap();
    let expected_collat_coeff = FixedI128::from_rational(
        balance + collat_ar.mul_int(debt_token_suply).unwrap(),
        s_token_supply,
    )
    .unwrap()
    .into_inner();

    let collat_coeff = sut.pool.collat_coeff(&debt_config.token.address);
    assert_eq!(collat_coeff, expected_collat_coeff);
    std::println!("collat_coeff={}", collat_coeff);
}

#[test]
#[should_panic(expected = "HostError: Error(Value, InvalidInput)")]
fn liquidity_cap_test() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);

    env.budget().reset_unlimited();

    let (lender, _borrower, debt_config) = fill_pool(&env, &sut);

    let token_one = 10_i128.pow(debt_config.token.decimals());
    let liq_bonus = 11000; //110%
    let liq_cap = 1_000_000 * 10_i128.pow(debt_config.token.decimals()); // 1M
    let discount = 6000; //60%
    let util_cap = 9000; //90%

    sut.pool.configure_as_collateral(
        &debt_config.token.address,
        &CollateralParamsInput {
            liq_bonus,
            liq_cap,
            discount,
            util_cap,
        },
    );

    //TODO: check error after soroban fix
    let deposit_amount = 1_000_000 * token_one;
    sut.pool
        .deposit(&lender, &debt_config.token.address, &deposit_amount);

    // assert_eq!(
    //     sut.pool
    //         .try_deposit(&lender, &debt_config.token.address, &deposit_amount)
    //         .unwrap_err()
    //         .unwrap(),
    //     Error::LiqCapExceeded
    // );
}

#[test]
fn stoken_balance_not_changed_when_direct_transfer_to_underlying_asset() {
    let env = Env::default();
    env.mock_all_auths();

    let sut = init_pool(&env);
    let lender = Address::random(&env);

    env.budget().reset_unlimited();

    sut.reserves[0].token_admin.mint(&lender, &2_000_000_000);
    sut.pool
        .deposit(&lender, &sut.reserves[0].token.address, &1_000_000_000);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[0].s_token.address);

    assert_eq!(s_token_underlying_supply, 1_000_000_000);

    sut.reserves[0]
        .token
        .transfer(&lender, &sut.reserves[0].s_token.address, &1_000_000_000);

    let s_token_underlying_supply = sut
        .pool
        .get_stoken_underlying_balance(&sut.reserves[0].s_token.address);

    assert_eq!(s_token_underlying_supply, 1_000_000_000);
}
