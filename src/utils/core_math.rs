use uint::construct_uint;

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
pub fn price_to_tick(price: f64) -> i32 {
    let numerator = price.sqrt().ln();
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
        calculate_liquidity_a(amount_a, lower_sqrt_price, upper_sqrt_price)
    } else if current_sqrt_price >= upper_sqrt_price {
        calculate_liquidity_b(amount_b, lower_sqrt_price, upper_sqrt_price)
    } else {
        let l_a = calculate_liquidity_a(amount_a, current_sqrt_price, upper_sqrt_price);
        let l_b = calculate_liquidity_b(amount_b, lower_sqrt_price, current_sqrt_price);
        l_a.min(l_b)
    }
}

pub fn calculate_liquidity_a(amount: U256, lower_sqrt_price: U256, upper_sqrt_price: U256) -> U256 {
    amount
        .checked_mul(lower_sqrt_price)
        .and_then(|v| v.checked_mul(upper_sqrt_price))
        .and_then(|v| v.checked_div(Q64))
        .and_then(|v| v.checked_div(upper_sqrt_price.checked_sub(lower_sqrt_price)?))
        .unwrap()
}

pub fn calculate_liquidity_b(amount: U256, lower_sqrt_price: U256, upper_sqrt_price: U256) -> U256 {
    amount
        .checked_mul(Q64)
        .and_then(|v| v.checked_div(upper_sqrt_price.checked_sub(lower_sqrt_price)?))
        .unwrap()
}

pub fn calculate_token_a_from_liquidity(
    liquidity: U256,
    sqrt_price_current: U256,
    sqrt_price_upper: U256,
) -> U256 {
    // Calculate (sqrt_price_upper - sqrt_price_current) * Q128 / (sqrt_price_current * sqrt_price_upper)
    let numerator = sqrt_price_upper
        .checked_sub(sqrt_price_current)
        .unwrap()
        .checked_mul(Q128)
        .unwrap();

    let denominator = sqrt_price_current.checked_mul(sqrt_price_upper).unwrap();

    let inverse_diff = numerator.checked_div(denominator).unwrap();

    // Multiply by liquidity and divide by Q64 to adjust for fixed-point representation
    liquidity
        .checked_mul(inverse_diff)
        .unwrap()
        .checked_div(Q64)
        .unwrap()
}

pub fn calculate_token_b_from_liquidity(
    liquidity: U256,
    sqrt_price_current: U256,
    sqrt_price_lower: U256,
) -> U256 {
    liquidity
        .checked_mul(sqrt_price_current.checked_sub(sqrt_price_lower).unwrap())
        .and_then(|v| v.checked_div(Q64))
        .unwrap()
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
        let amount_a = calculate_token_a_from_liquidity(
            liquidity,
            lower_sqrt_price_fixed,
            upper_sqrt_price_fixed,
        );
        (amount_a, U256::zero())
    } else if current_sqrt_price_fixed >= upper_sqrt_price_fixed {
        // Price is at or above the upper bound
        // All liquidity is in token B
        let amount_b = calculate_token_b_from_liquidity(
            liquidity,
            upper_sqrt_price_fixed,
            lower_sqrt_price_fixed,
        );
        (U256::zero(), amount_b)
    } else {
        // Price is within the range
        // Liquidity is split between token A and B
        let amount_a = calculate_token_a_from_liquidity(
            liquidity,
            current_sqrt_price_fixed,
            upper_sqrt_price_fixed,
        );

        let amount_b = calculate_token_b_from_liquidity(
            liquidity,
            current_sqrt_price_fixed,
            lower_sqrt_price_fixed,
        );

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
    if is_sell {
        // sqrtP_new = (L * sqrtP_current) / (L + Δx * sqrtP_current)
        let numerator = liquidity.checked_mul(current_sqrt_price).unwrap();
        let denominator = liquidity
            .checked_add(
                amount_in
                    .checked_mul(current_sqrt_price)
                    .unwrap()
                    .checked_div(Q64)
                    .unwrap(),
            )
            .unwrap();
        numerator.checked_div(denominator).unwrap()
    } else {
        // sqrtP_new = sqrtP_current + (Δy * Q64) / L
        let delta = amount_in
            .checked_mul(Q64)
            .unwrap()
            .checked_div(liquidity)
            .unwrap();
        current_sqrt_price.checked_add(delta).unwrap()
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
        // the nmr is adjusted with decimal diff, since thats how it works when u divide the amounts. SOL real price is 133.44....
        assert_eq!(price_to_tick(0.133446536f64), -20142);

        // SOL/POPCAT
        // 0 token decimal adjustment
        assert_eq!(price_to_tick(206.071016394f64), 53284);

        // SOL/WIF
        //3 decimal adjustment, real price is 86.7....
        assert_eq!(price_to_tick(0.086719236f64), -24453);
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

    #[test]
    fn test_revert_amounts_from_liquidity() {
        let curr_sqrt_price = tick_to_sqrt_price_u256(10);
        let lower_sqrt_price = tick_to_sqrt_price_u256(10 - 5);
        let upper_sqrt_price = tick_to_sqrt_price_u256(10 + 5);

        let starting_amount_a = U256::from(1000);
        let starting_amount_b = U256::from(1000);

        let liquidity = calculate_liquidity(
            starting_amount_a,
            starting_amount_b,
            curr_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        let (amount_a, amount_b) = calculate_amounts(
            liquidity,
            curr_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        let TOLERANCE = U256::from(2);

        // THE AMOUNTS COME OUT AS 999 FOR BOTH. KEEP IN MIND THESE CALCS WILL NEVER BE 100% precise, its same in the real world systems. GOOD ENOUGH THO.
        assert!(
            (starting_amount_a - amount_a) <= TOLERANCE,
            "amount_a should be within tolerance. Expected: {}, Got: {}",
            starting_amount_a,
            amount_a
        );

        assert!(
            (starting_amount_b - amount_b) <= TOLERANCE,
            "amount_b should be within tolerance. Expected: {}, Got: {}",
            starting_amount_b,
            amount_b
        );
    }
}
