use uint::construct_uint;

use super::error::PriceCalcError;

construct_uint! {
    pub struct U256(4);
}

pub const Q64: U256 = U256([0, 1, 0, 0]);
pub const Q128: U256 = U256([0, 0, 1, 0]);

// WORKS WITHIN A REASONABLE LIMIT. TESTED AGAINST LIVE STUFF.
pub fn tick_to_sqrt_price_u256(tick: i32) -> U256 {
    let sqrt_price = (1.0001_f64.powf(tick as f64 / 2.0)) * (Q64.as_u128() as f64);
    U256::from(sqrt_price as u128)
}

// WORKS GREAT. DO NOT TOUCH. ACCURATE. TESTED AGAINST LIVE SWAPS.
pub fn price_to_tick(price: f64, decimal_diff: i16) -> i32 {
    // Step 1: Convert price to sqrtPrice
    let sqrt_price = (price / 10_f64.powf(decimal_diff as f64)).sqrt();

    let numerator = sqrt_price.ln();
    let denominator = 1.0001f64.ln();

    (2.0 * numerator / denominator).floor() as i32
}
// THIS FUNCTION WORKS. TESTED AGAINST LIVE POSITIONS.
pub fn calculate_liquidity(
    amount_a: U256,
    amount_b: U256,
    current_sqrt_price: U256,
    lower_sqrt_price: U256,
    upper_sqrt_price: U256,
) -> U256 {
    if current_sqrt_price <= lower_sqrt_price {
        // All liquidity is in token A
        amount_a
            .checked_mul(lower_sqrt_price)
            .and_then(|v| v.checked_mul(upper_sqrt_price))
            .and_then(|v| v.checked_div(Q64))
            .and_then(|v| v.checked_div(upper_sqrt_price.checked_sub(lower_sqrt_price)?))
            .unwrap()
    } else if current_sqrt_price >= upper_sqrt_price {
        // All liquidity is in token B
        amount_b
            .checked_mul(Q64)
            .and_then(|v| v.checked_div(upper_sqrt_price.checked_sub(lower_sqrt_price)?))
            .unwrap()
    } else {
        // Price is within the range
        let l_a = amount_a
            .checked_mul(Q64)
            .and_then(|v| v.checked_div(upper_sqrt_price.checked_sub(current_sqrt_price)?))
            .and_then(|v| v.checked_mul(current_sqrt_price))
            .and_then(|v| v.checked_div(Q64))
            .unwrap();

        let l_b = amount_b
            .checked_mul(Q64)
            .and_then(|v| v.checked_div(current_sqrt_price.checked_sub(lower_sqrt_price)?))
            .unwrap();

        l_a.min(l_b)
    }
}

// THIS FUNCTION WORKS. TESTED AGAINST LIVE POSITIONS.
pub fn calculate_amounts(
    liquidity: U256,
    current_sqrt_price_fixed: U256,
    lower_sqrt_price_fixed: U256,
    upper_sqrt_price_fixed: U256,
) -> (U256, U256) {
    if current_sqrt_price_fixed <= lower_sqrt_price_fixed {
        // Price is at or below the lower bound
        // All liquidity is in token A
        let amount_a = (liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed) * Q64)
            / (lower_sqrt_price_fixed * upper_sqrt_price_fixed);
        (amount_a, U256::zero())
    } else if current_sqrt_price_fixed >= upper_sqrt_price_fixed {
        // Price is at or above the upper bound
        // All liquidity is in token B
        let amount_b = liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed) / Q64;
        (U256::zero(), amount_b)
    } else {
        // Price is within the range
        // Liquidity is split between token A and B
        let amount_a = liquidity * (upper_sqrt_price_fixed - current_sqrt_price_fixed) * Q64
            / (current_sqrt_price_fixed * upper_sqrt_price_fixed);

        let amount_b = liquidity * (current_sqrt_price_fixed - lower_sqrt_price_fixed) / Q64;

        (amount_a, amount_b)
    }
}

// General formulas:
// amount_a changing: sqrt_P_new = (sqrt_P * L) / (L + Δx * sqrt_P)
// amount_b changing: sqrt_P_new = sqrt_P + (Δy / L)
pub fn calculate_new_sqrt_price(
    current_sqrt_price: U256,
    liquidity: U256,
    amount_in: U256,
    is_sell: bool,
) -> U256 {
    // Formula explanations for later in case need to edit:
    // x = L / sqrt(P) also y = L * sqrt(P)
    if is_sell {
        /*
        for this case:
        (x + Δx) * y = L^2

        (L/sqrt(P) + Δx) * (L*sqrt(P)) = L^2
        L^2 + Δx*L*srt(P)q = L^2
        Δx*L*sqrt(P) = L^2 - L^2 = 0

        After price change we must satisfy: (L/sqrt(P_new)) * (L*sqrt(P_new)) = L^2.
        Hence, L/sqrt(P_new) = L/sqrt(P) + Δx. sqrt(P_new) = L / (L/sqrt(P) + Δx).

        sqrt(P_new) = (L * sqrt(P)) / (L + Δx * sqrt(P))
        */
        let numerator = current_sqrt_price.checked_mul(liquidity).unwrap();
        let product = amount_in.checked_mul(current_sqrt_price).unwrap();
        let denominator = liquidity
            .checked_add(product.checked_div(Q64).unwrap())
            .unwrap();
        numerator.checked_div(denominator).unwrap()
    } else {
        /*
        for this case:
        x * (y + Δy) = L^2

        (L/sqrt(P)) * (L*sqrt(P) + Δy) = L^2
        L^2 + L*Δy = L^2
        L*Δy = L^2 - L^2 = 0

        After price change we must satisfy: (L/sqrt(P_new)) * (L*sqrt(P_new)) = L^2.
        Hence, L*sqrt(P_new) = L*sqrt(P) + Δy. sqrt(P_new) = sqrt(P) + (Δy / L).

        sqrt(P_new) = sqrt(P) + (Δy / L)
        */
        let increment = amount_in
            .checked_mul(Q64)
            .unwrap()
            .checked_div(liquidity)
            .unwrap();
        current_sqrt_price.checked_add(increment).unwrap()
    }
}

pub fn calculate_rebalance_amount(
    amount_a: U256,
    amount_b: U256,
    current_sqrt_price: U256,
    rebalance_ratio: U256, // Use U256 for ratio, where 1.0 = Q64
) -> (U256, bool) {
    // Calculate total value in terms of token A. We multiply amount_b by Q32 (even tho its scaled alrdy) to unscale the sqrt price.
    let total_value_a = amount_a + (amount_b * Q64 / current_sqrt_price);

    // Calculate target amount of token A
    let target_amount_a = total_value_a * rebalance_ratio / Q64;

    if amount_a > target_amount_a {
        // Need to sell token A
        (amount_a - target_amount_a, true)
    } else {
        // Need to sell token B
        let target_amount_b =
            (total_value_a * (Q64 - rebalance_ratio) / Q64) * current_sqrt_price / Q64;
        (amount_b - target_amount_b, false)
    }
}

// THE LIQUIDITY AND AMOUNTS CALCULATIONS ARE CHECKED ON SAME POSITIONS AGAINST EACH OTHER.
#[cfg(test)]
mod tests {
    use super::*;

    fn sqrt_price_to_u256(sqrt_price: f64) -> U256 {
        let scaled_sqrt_price = (sqrt_price * Q64.as_u128() as f64) as u128;
        U256::from(scaled_sqrt_price)
    }

    fn calculate_relative_error(expected: U256, actual: U256) -> f64 {
        let diff = if expected > actual {
            expected - actual
        } else {
            actual - expected
        };
        (diff.as_u128() as f64) / (expected.as_u128() as f64)
    }

    #[test]
    fn test_tick_to_sqrt_price() {
        // Not able to get it lower than this. We will have to live with it :(. 0.0001% error.
        let acceptable_error = 1e-4;

        // SOL/USDC.
        let result = tick_to_sqrt_price_u256(-19998);
        let expected = U256::from(6787344857950480093_u128);
        let error = calculate_relative_error(expected, result);
        assert!(
            error <= acceptable_error,
            "Error {} exceeds acceptable error {}",
            error,
            acceptable_error
        );

        // POPCAT/SOL.
        let result = tick_to_sqrt_price_u256(53249);
        let expected = U256::from(264342069548887880143_u128);
        let error = calculate_relative_error(expected, result);
        assert!(
            error <= acceptable_error,
            "Error {} exceeds acceptable error {}",
            error,
            acceptable_error
        );

        // WIF/SOL.
        let result = tick_to_sqrt_price_u256(-24286);
        let expected = U256::from(5477672977344760390_u128);
        let error = calculate_relative_error(expected, result);
        assert!(
            error <= acceptable_error,
            "Error {} exceeds acceptable error {}",
            error,
            acceptable_error
        );
    }

    #[test]
    fn test_sqrt_price_to_tick() {
        // SOL_USDC
        assert_eq!(price_to_tick(133.446536f64, 3), -20142);

        // SOL/POPCAT
        assert_eq!(price_to_tick(206.071016394f64, 0), 53284);

        // SOL/WIF
        assert_eq!(price_to_tick(86.719236f64, 3), -24453);
    }

    #[test]
    fn test_calculate_new_sqrt_price() {
        let liquidity = U256::from(1_000_000_000_000_u128);
        let current_sqrt_price = sqrt_price_to_u256(1.0);
        let amount_in = sqrt_price_to_u256(10.0);

        // Test sell
        let new_sqrt_price_sell =
            calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, true);

        assert!(
            new_sqrt_price_sell < current_sqrt_price,
            "Sell should decrease price"
        );

        // Test buy
        let new_sqrt_price_buy =
            calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, false);

        assert!(
            new_sqrt_price_buy > current_sqrt_price,
            "Buy should increase price"
        );
    }

    #[test]
    fn test_calculate_amounts() {
        // Case 4 (live): Price is outside of range (all in token a)
        let (amount_a, amount_b) = calculate_amounts(
            U256::from(123197299862_u128),
            tick_to_sqrt_price_u256(-19944),
            tick_to_sqrt_price_u256(-17204),
            tick_to_sqrt_price_u256(-16446),
        );

        assert!(
            amount_b == U256::zero(),
            "Amount B should be zero when price is at lower bound"
        );
        // amount a
        assert!(
            calculate_relative_error(U256::from(10828707975_u128), amount_a) <= 1e-5,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(10828707975_u128), amount_a),
            1e-5
        );

        // Case 5 (live): Price is in range
        let (amount_a, amount_b) = calculate_amounts(
            U256::from(9913435703877_u128),
            tick_to_sqrt_price_u256(-19981),
            tick_to_sqrt_price_u256(-20164),
            tick_to_sqrt_price_u256(-16096),
        );

        // amount a
        assert!(
            calculate_relative_error(U256::from(4751690281711_u128), amount_a) <= 1e-3,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(4751690281711_u128), amount_a),
            1e-3
        );

        // amount b
        assert!(
            calculate_relative_error(U256::from(33366735075_u128), amount_b) <= 1e-2,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(33366735075_u128), amount_b),
            1e-2
        );

        // Case 6 (live): Price out of range, all token b.
        let (amount_a, amount_b) = calculate_amounts(
            U256::from(54643495974_u128),
            tick_to_sqrt_price_u256(-19985),
            tick_to_sqrt_price_u256(-20640),
            tick_to_sqrt_price_u256(-20536),
        );

        assert!(
            amount_a == U256::zero(),
            "Amount A should be zero when price is at lower bound"
        );
        // amount b
        assert!(
            calculate_relative_error(U256::from(101503310_u128), amount_b) <= 1e-5,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(101503310_u128), amount_b),
            1e-5
        );
    }

    #[test]
    fn test_calculate_rebalance_amount() {
        // Test case: Need to sell token A
        {
            let amount_a = U256::from(1500);
            let amount_b = U256::from(500);
            let current_sqrt_price = sqrt_price_to_u256(1.0);
            let rebalance_ratio = Q64 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > U256::zero(), "Should sell some token A");
            assert_eq!(is_sell_a, true, "Should sell token A");
            assert!(
                amount_to_sell <= U256::from(500),
                "Should not sell more than the imbalance"
            );
        }

        // Test case: Need to sell token B
        {
            let amount_a = U256::from(500);
            let amount_b = U256::from(1500);
            let current_sqrt_price = sqrt_price_to_u256(1.0);
            let rebalance_ratio = Q64 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > U256::zero(), "Should sell some token B");
            assert_eq!(is_sell_a, false, "Should sell token B");
            assert!(
                amount_to_sell <= U256::from(500),
                "Should not sell more than the imbalance"
            );
        }

        // Test case 4: Rebalance to 60/40
        {
            let amount_a = U256::from(1000);
            let amount_b = U256::from(1000);
            let current_sqrt_price = sqrt_price_to_u256(1.0);
            let rebalance_ratio = U256::from(6) * Q64 / 10; // 60%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > U256::zero(), "Should sell some token B");
            assert_eq!(is_sell_a, false, "Should sell token B");
            assert!(
                amount_to_sell <= U256::from(200),
                "Should sell approximately 10% of token B"
            );
        }

        // Test case: Edge case - all token A
        {
            let amount_a = U256::from(1000);
            let amount_b = U256::zero();
            let current_sqrt_price = sqrt_price_to_u256(1.0);
            let rebalance_ratio = Q64 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > U256::zero(), "Should sell some token A");
            assert!(
                amount_to_sell <= U256::from(500),
                "Should sell approximately half of token A"
            );
        }

        // Test case: Edge case - all token B
        {
            let amount_a = U256::zero();
            let amount_b = U256::from(1000);
            let current_sqrt_price = sqrt_price_to_u256(1.0);
            let rebalance_ratio = Q64 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > U256::zero(), "Should sell some token B");
            assert!(
                amount_to_sell <= U256::from(500) * Q64,
                "Should sell approximately half of token B"
            );
        }
    }

    #[test]
    fn test_calculate_liquidity() {
        let amount_a = U256::from(1000); // 1000 tokens
        let amount_b = U256::from(1000); // 1000 tokens
        let lower_sqrt_price = sqrt_price_to_u256(0.99);
        let upper_sqrt_price = sqrt_price_to_u256(1.01);

        // Case 4: Edge case test
        let current_sqrt_price = sqrt_price_to_u256(1.02);
        let liquidity = calculate_liquidity(
            U256::from(5_000_000 * 10_u128.pow(9)),
            U256::from(5_000_000 * 10_u128.pow(9)),
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        let expected =
            U256::from(5_000_000 * 10_u128.pow(9)) * Q64 / (upper_sqrt_price - lower_sqrt_price);
        assert_eq!(
            liquidity, expected,
            "When price is above range, liquidity should be based on token B"
        );

        // Case 5 (live): Out of range, all token A.
        let liquidity = calculate_liquidity(
            U256::from(10828707975_u128),
            U256::zero(),
            tick_to_sqrt_price_u256(-19944),
            tick_to_sqrt_price_u256(-17204),
            tick_to_sqrt_price_u256(-16446),
        );

        assert!(
            calculate_relative_error(U256::from(123197299862_u128), liquidity) <= 1e-5,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(123197299862_u128), liquidity),
            1e-5
        );

        // Case 6 (live): Out of range, all token B.
        let liquidity = calculate_liquidity(
            U256::zero(),
            U256::from(101503310_u128),
            tick_to_sqrt_price_u256(-19985),
            tick_to_sqrt_price_u256(-20640),
            tick_to_sqrt_price_u256(-20536),
        );

        assert!(
            calculate_relative_error(U256::from(54643495974_u128), liquidity) <= 1e-5,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(54643495974_u128), liquidity),
            1e-5
        );

        // Case 7 (live): In range, both token A and B.
        let liquidity = calculate_liquidity(
            U256::from(4751690281711_u128),
            U256::from(33366735075_u128),
            tick_to_sqrt_price_u256(-19981),
            tick_to_sqrt_price_u256(-20164),
            tick_to_sqrt_price_u256(-16096),
        );

        assert!(
            calculate_relative_error(U256::from(9913435703877_u128), liquidity) <= 1e-2,
            "Error {} exceeds acceptable error {}",
            calculate_relative_error(U256::from(9913435703877_u128), liquidity),
            1e-2
        );
    }

    #[test]
    fn test_to_show_how_dynamic_liquidity_is() {
        let (amount_a, _) = calculate_amounts(
            U256::from(9913435703877_u128),
            tick_to_sqrt_price_u256(-21000),
            tick_to_sqrt_price_u256(-20164),
            tick_to_sqrt_price_u256(-16096),
        );

        assert_eq!(
            U256::from(4999),
            U256::from(amount_a / 10_i32.pow(9)),
            "amount_a match when below lower range"
        );

        let (_, amount_b) = calculate_amounts(
            U256::from(9913435703877_u128),
            tick_to_sqrt_price_u256(-15000),
            tick_to_sqrt_price_u256(-20164),
            tick_to_sqrt_price_u256(-16096),
        );

        assert_eq!(
            U256::from(815893),
            U256::from(amount_b / 10_i32.pow(6)),
            "amount_a match when below lower range"
        );

        //2nd example.

        let starting_tick = -19969;
        let lower_tick = -20000;
        let upper_tick = -17000;

        let starting_sqrt_price_u256 = tick_to_sqrt_price_u256(starting_tick);

        let liquidity = calculate_liquidity(
            U256::from(500 * 10_u128.pow(9)),
            U256::from(67884 * 10_u128.pow(6)),
            starting_sqrt_price_u256,
            tick_to_sqrt_price_u256(lower_tick),
            tick_to_sqrt_price_u256(upper_tick),
        );

        let starting_tick = -16000;
        let lower_tick = -20000;
        let upper_tick = -17000;

        let starting_sqrt_price_u256 = tick_to_sqrt_price_u256(starting_tick);

        let liquidity = calculate_liquidity(
            U256::from(500 * 10_u128.pow(9)),
            U256::from(67884 * 10_u128.pow(6)),
            starting_sqrt_price_u256,
            tick_to_sqrt_price_u256(lower_tick),
            tick_to_sqrt_price_u256(upper_tick),
        );

        let starting_tick = -21000;
        let lower_tick = -20000;
        let upper_tick = -17000;

        let starting_sqrt_price_u256 = tick_to_sqrt_price_u256(starting_tick);

        let liquidity = calculate_liquidity(
            U256::from(500 * 10_u128.pow(9)),
            U256::from(67884 * 10_u128.pow(6)),
            starting_sqrt_price_u256,
            tick_to_sqrt_price_u256(lower_tick),
            tick_to_sqrt_price_u256(upper_tick),
        );
    }
}
