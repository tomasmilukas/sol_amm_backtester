use crate::try_calc;

use super::error::PriceCalcError;

pub const Q32: u128 = 1u128 << 32;

pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001f64.powf(tick as f64 / 2.0)
}

pub fn sqrt_price_to_tick(sqrt_price: f64) -> i32 {
    ((sqrt_price.ln() / 1.0001f64.ln()) * 2.0).floor() as i32
}

pub fn sqrt_price_to_fixed(sqrt_price: f64) -> u128 {
    (sqrt_price * Q32 as f64) as u128
}

pub fn calculate_correct_liquidity(
    amount_a: u128,
    amount_b: u128,
    current_sqrt_price: u128,
    lower_sqrt_price: u128,
    upper_sqrt_price: u128,
) -> u128 {
    // Calculate liquidity for amount A
    let liquidity_a = if current_sqrt_price <= lower_sqrt_price {
        0
    } else if current_sqrt_price < upper_sqrt_price {
        amount_a * (current_sqrt_price * lower_sqrt_price / Q32) / (current_sqrt_price - lower_sqrt_price)
    } else {
        amount_a * lower_sqrt_price / Q32
    };

    // Calculate liquidity for amount B
    let liquidity_b = if current_sqrt_price <= lower_sqrt_price {
        amount_b * Q32 / (upper_sqrt_price - lower_sqrt_price)
    } else if current_sqrt_price < upper_sqrt_price {
        amount_b * Q32 / (upper_sqrt_price - current_sqrt_price)
    } else {
        0
    };

    // Return the minimum of the two calculated liquidities
    liquidity_a.min(liquidity_b)
}

// inversed from formulas since the arrangement is different. check calculate amounts or new sqrt price calculation for full logic details.
pub fn calculate_liquidity_for_amount_a(
    amount: u128,
    current_sqrt_price: u128,
    lower_sqrt_price: u128,
) -> u128 {
    if current_sqrt_price <= lower_sqrt_price {
        return 0;
    }
    (amount * current_sqrt_price * lower_sqrt_price / Q32) / (current_sqrt_price - lower_sqrt_price)
}

pub fn calculate_liquidity_for_amount_b(
    amount: u128,
    current_sqrt_price: u128,
    upper_sqrt_price: u128,
) -> u128 {
    if current_sqrt_price >= upper_sqrt_price {
        return 0;
    }
    (amount * Q32) / (upper_sqrt_price - current_sqrt_price)
}

pub fn calculate_amounts(
    liquidity: u128,
    current_sqrt_price_fixed: u128,
    lower_sqrt_price_fixed: u128,
    upper_sqrt_price_fixed: u128,
) -> (u128, u128) {
    // We calculate amounts based on the position of current_sqrt_price relative to the range

    if current_sqrt_price_fixed <= lower_sqrt_price_fixed {
        // Price is at or below the lower bound
        // All liquidity is in token B
        let amount_b = (liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed)) / Q32;

        (0, amount_b)
    } else if current_sqrt_price_fixed >= upper_sqrt_price_fixed {
        // Price is at or above the upper bound
        // All liquidity is in token A
        let amount_a = (liquidity * Q32 / lower_sqrt_price_fixed)
            .checked_sub(liquidity * Q32 / upper_sqrt_price_fixed)
            .unwrap();

        (amount_a, 0)
    } else {
        // Price is within the range
        // Liquidity is split between token A and B
        // FYI formulas re-arranged from official docs. I think they got it wrong. Used p_u-p_c for a amount which reflects b amount. idk... my math looks solid when testing it.

        let amount_a = (liquidity * Q32 / lower_sqrt_price_fixed)
            .checked_sub(liquidity * Q32 / current_sqrt_price_fixed)
            .unwrap();

        // Amount of token B: L * (sqrt(P_u) - sqrt(P_c))
        let amount_b = (liquidity * (upper_sqrt_price_fixed - current_sqrt_price_fixed)) / Q32;

        (amount_a, amount_b)
    }
}

// General formulas:
// amount_a changing: sqrt_P_new = (sqrt_P * L) / (L + Δx * sqrt_P)
// amount_b changing: sqrt_P_new = sqrt_P + (Δy / L)
pub fn calculate_new_sqrt_price(
    current_sqrt_price: u128,
    liquidity: u128,
    amount_in: u128,
    is_sell: bool,
) -> Result<u128, PriceCalcError> {
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
        let numerator = try_calc!(current_sqrt_price.checked_mul(liquidity))?;
        let product = try_calc!(amount_in.checked_mul(current_sqrt_price))?;
        let denominator = try_calc!(liquidity.checked_add(product / Q32))?;
        try_calc!(numerator.checked_div(denominator))
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
        let product = try_calc!(amount_in.checked_mul(Q32))?;
        let increment = try_calc!(product.checked_div(liquidity))?;
        try_calc!(current_sqrt_price.checked_add(increment))
    }
}

pub fn calculate_rebalance_amount(
    amount_a: u128,
    amount_b: u128,
    current_sqrt_price: u128,
    rebalance_ratio: u128, // Use u128 for ratio, where 1.0 = Q32
) -> (u128, bool) {
    // Calculate total value in terms of token A. We multiply amount_b by Q32 (even tho its scaled alrdy) to unscale the sqrt price.
    let total_value_a = amount_a + (amount_b * Q32 / current_sqrt_price);

    // Calculate target amount of token A
    let target_amount_a = total_value_a * rebalance_ratio / Q32;

    if amount_a > target_amount_a {
        // Need to sell token A
        (amount_a - target_amount_a, true)
    } else {
        // Need to sell token B
        let target_amount_b =
            (total_value_a * (Q32 - rebalance_ratio) / Q32) * current_sqrt_price / Q32;
        (amount_b - target_amount_b, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_new_sqrt_price() {
        let liquidity = 1_000_000_000_000;
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let amount_in = sqrt_price_to_fixed(10.0);

        // Test sell
        let new_sqrt_price_sell =
            calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, true).unwrap();

        assert!(
            new_sqrt_price_sell < current_sqrt_price,
            "Sell should decrease price"
        );

        // Test buy
        let new_sqrt_price_buy =
            calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, false).unwrap();
        assert!(
            new_sqrt_price_buy > current_sqrt_price,
            "Buy should increase price"
        );
    }

    #[test]
    fn test_calculate_amounts() {
        let liquidity = 1_000_000_000_000;
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let lower_sqrt_price = sqrt_price_to_fixed(0.99);
        let upper_sqrt_price = sqrt_price_to_fixed(1.01);

        // Equal 50/50 case.
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a > 99 / 10 * 10_i32.pow(9) as u128
                && amount_a <= 110 / 10 * 10_i32.pow(9) as u128
                && amount_b > 99 / 10 * 10_i32.pow(9) as u128
                && amount_b <= 100 * 10_i32.pow(9) as u128,
            "Both amounts close to 50/50"
        );

        // 100/0 case.
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            lower_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_b >= 19 * 10_i32.pow(9) as u128 && amount_a == 0,
            "amountb is full and a is 0"
        );

        // 0/100 case.
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            upper_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a >= 19 * 10_i32.pow(9) as u128 && amount_b == 0,
            "amount a is full and b is 0"
        );
    }

    #[test]
    fn test_calculate_rebalance_amount() {
        // Test case: Need to sell token A
        {
            let amount_a = 1500 * Q32;
            let amount_b = 500 * Q32;
            let current_sqrt_price = sqrt_price_to_fixed(1.0);
            let rebalance_ratio = Q32 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > 0, "Should sell some token A");
            assert_eq!(is_sell_a, true, "Should sell token A");
            assert!(
                amount_to_sell <= 500 * Q32,
                "Should not sell more than the imbalance"
            );
        }

        // Test case: Need to sell token B
        {
            let amount_a = 500 * Q32;
            let amount_b = 1500 * Q32;
            let current_sqrt_price = sqrt_price_to_fixed(1.0);
            let rebalance_ratio = Q32 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > 0, "Should sell some token B");
            assert_eq!(is_sell_a, false, "Should sell token B");
            assert!(
                amount_to_sell <= 500 * Q32,
                "Should not sell more than the imbalance"
            );
        }

        // Test case 4: Rebalance to 60/40
        {
            let amount_a = 1000 * Q32;
            let amount_b = 1000 * Q32;
            let current_sqrt_price = sqrt_price_to_fixed(1.0);
            let rebalance_ratio = 6 * Q32 / 10; // 60%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > 0, "Should sell some token B");
            assert_eq!(is_sell_a, false, "Should sell token B");
            assert!(
                amount_to_sell <= 200 * Q32,
                "Should sell approximately 10% of token B"
            );
        }

        // Test case: Edge case - all token A
        {
            let amount_a = 1000 * Q32;
            let amount_b = 0;
            let current_sqrt_price = sqrt_price_to_fixed(1.0);
            let rebalance_ratio = Q32 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > 0, "Should sell some token A");
            assert_eq!(is_sell_a, true, "Should sell token A");
            assert!(
                amount_to_sell <= 500 * Q32,
                "Should sell approximately half of token A"
            );
        }

        // Test case: Edge case - all token B
        {
            let amount_a = 0;
            let amount_b = 1000 * Q32;
            let current_sqrt_price = sqrt_price_to_fixed(1.0);
            let rebalance_ratio = Q32 / 2; // 50%

            let (amount_to_sell, is_sell_a) =
                calculate_rebalance_amount(amount_a, amount_b, current_sqrt_price, rebalance_ratio);

            assert!(amount_to_sell > 0, "Should sell some token B");
            assert_eq!(is_sell_a, false, "Should sell token B");
            assert!(
                amount_to_sell <= 500 * Q32,
                "Should sell approximately half of token B"
            );
        }
    }
}
