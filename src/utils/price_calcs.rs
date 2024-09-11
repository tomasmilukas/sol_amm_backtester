use crate::try_calc;

use super::error::PriceCalcError;

pub const Q32: u128 = 1u128 << 32;
pub const Q64: u128 = 1u128 << 64;

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
    let liquidity_a = if current_sqrt_price <= lower_sqrt_price {
        amount_a * (upper_sqrt_price - lower_sqrt_price) / Q32
    } else if current_sqrt_price < upper_sqrt_price {
        amount_a * (upper_sqrt_price - current_sqrt_price) / Q32
    } else {
        0
    };

    let liquidity_b = if current_sqrt_price <= lower_sqrt_price {
        0
    } else if current_sqrt_price < upper_sqrt_price {
        amount_b * Q32 / (upper_sqrt_price - current_sqrt_price)
    } else {
        amount_b * Q32 / (upper_sqrt_price - lower_sqrt_price)
    };

    if liquidity_a == 0 {
        liquidity_b
    } else if liquidity_b == 0 {
        liquidity_a
    } else {
        liquidity_a.min(liquidity_b)
    }
}

pub fn calculate_amounts(
    liquidity: u128,
    current_sqrt_price_fixed: u128,
    lower_sqrt_price_fixed: u128,
    upper_sqrt_price_fixed: u128,
) -> (u128, u128) {
    if current_sqrt_price_fixed <= lower_sqrt_price_fixed {
        // Price is at or below the lower bound
        // All liquidity is in token A
        let amount_a = liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed) / Q32;
        (amount_a, 0)
    } else if current_sqrt_price_fixed >= upper_sqrt_price_fixed {
        // Price is at or above the upper bound
        // All liquidity is in token B
        let amount_b = liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed) / Q32;
        (0, amount_b)
    } else {
        // Price is within the range
        // Liquidity is split between token A and B
        let amount_a = liquidity * (upper_sqrt_price_fixed - current_sqrt_price_fixed) / Q32;
        let amount_b = liquidity * (current_sqrt_price_fixed - lower_sqrt_price_fixed) / Q32;
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

        // Case 1: Price in the middle of the range (approximately 50/50 split)
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a > 0 && amount_b > 0,
            "Both amounts should be non-zero when price is in range"
        );
        assert!(
            (amount_a as f64 / amount_b as f64 - 1.0).abs() < 0.1,
            "Amounts should be roughly equal when price is in the middle"
        );

        // Case 2: Price at lower bound (100% token A, 0% token B)
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            lower_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a > 0 && amount_b == 0,
            "All liquidity should be in token A when price is at lower bound"
        );

        // Case 3: Price at upper bound (0% token A, 100% token B)
        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            upper_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a == 0 && amount_b > 0,
            "All liquidity should be in token B when price is at upper bound"
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

    #[test]
    fn test_calculate_correct_liquidity() {
        let amount_a = 1000 * Q32;
        let amount_b = 1000 * Q32;
        let lower_sqrt_price = sqrt_price_to_fixed(0.99);
        let upper_sqrt_price = sqrt_price_to_fixed(1.01);

        // Case 1: Price below the range
        let current_sqrt_price = sqrt_price_to_fixed(0.98);
        let liquidity = calculate_correct_liquidity(
            amount_a,
            amount_b,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            liquidity > 0,
            "Liquidity should be non-zero when price is below the range"
        );

        let expected_liquidity = amount_a * (upper_sqrt_price - lower_sqrt_price) / Q32;
        assert_eq!(
            liquidity, expected_liquidity,
            "Liquidity should be calculated correctly when price is below the range"
        );

        // Case 2: Price within the range
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let liquidity = calculate_correct_liquidity(
            amount_a,
            amount_b,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );
        assert!(
            liquidity > 0,
            "Liquidity should be non-zero when price is within the range"
        );

        // Calculate expected liquidity for both tokens
        let liquidity_a = amount_a * (upper_sqrt_price - current_sqrt_price) / Q32;
        let liquidity_b = amount_b * Q32 / (upper_sqrt_price - current_sqrt_price);

        // The actual liquidity should be the minimum of these two
        let expected_liquidity = liquidity_a.min(liquidity_b);

        assert_eq!(
        liquidity,
        expected_liquidity,
        "Liquidity should be the minimum of liquidity_a and liquidity_b when price is within the range"
    );

        // Case 3: Price above the range
        let current_sqrt_price = sqrt_price_to_fixed(1.02);
        let liquidity = calculate_correct_liquidity(
            amount_a,
            amount_b,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );
        assert!(
            liquidity > 0,
            "Liquidity should be non-zero when price is above the range"
        );

        let expected_liquidity = amount_b * Q32 / (upper_sqrt_price - lower_sqrt_price);

        assert_eq!(
            liquidity, expected_liquidity,
            "Liquidity should be calculated correctly when price is above the range"
        );

        // Case 4: Uneven amounts
        let amount_a = 1500 * Q32;
        let amount_b = 500 * Q32;
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let liquidity = calculate_correct_liquidity(
            amount_a,
            amount_b,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );
        assert!(
            liquidity > 0,
            "Liquidity should be non-zero with uneven amounts"
        );

        let liquidity_a = amount_a * (upper_sqrt_price - current_sqrt_price) / Q32;
        let liquidity_b = amount_b * Q32 / (upper_sqrt_price - current_sqrt_price);

        let expected_liquidity = liquidity_a.min(liquidity_b);

        assert_eq!(
            liquidity, expected_liquidity,
            "Liquidity should be the minimum of liquidity_a and liquidity_b with uneven amounts"
        );

        assert!(
            liquidity < amount_a / (upper_sqrt_price - lower_sqrt_price) * Q32,
            "Liquidity should be less than amount_a when converted to the same units"
        );
        assert!(
            liquidity < amount_b * Q32 / (upper_sqrt_price - lower_sqrt_price),
            "Liquidity should be less than amount_b when converted to the same units"
        );
    }
}
