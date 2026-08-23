// Parsing rounds exactly (BigInt, half away from zero). Multiplication and division still
// truncate to two decimal places — see the "Limitations" section in the crate docs.

// Copyright (c) 2016 Tyler Berry All Rights Reserved.
//
// Licensed under the MIT license <LICENSE-MIT or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

//! A `Currency` is a combination of a symbol (`String`) and a big integer (`BigInt`) count of
//! coins. The symbol is a string, not a single character: this fork exists so that multi-character
//! codes like `"USD"` work alongside `'$'`.
//!
//! Common operations are overloaded to make numerical operations easy.
//!
//! Perhaps the most useful part of this crate is the `Currency::from_str` function, which can
//! convert international currency representations such as "$1,000.42" and "£10,99" into a
//! usable `Currency` instance.
//!
//! ## Example
//!
//! ```
//! extern crate qsv_currency;
//!
//! fn main() {
//!     use qsv_currency::Currency;
//!
//!     let sock_price = Currency::from_str("$11.99").unwrap();
//!     let toothbrush_price = Currency::from_str("$1.99").unwrap();
//!     let subtotal = sock_price + toothbrush_price;
//!     let tax_rate = 0.07;
//!     let total = &subtotal + (&subtotal * tax_rate);
//!     assert_eq!(format!("{}", total), "$14.95");
//! }
//! ```
//!
//! ## Limitations
//!
//! This crate cannot lookup conversion data dynamically. It does supply a `convert` function, but
//! the conversion rates will need to be input by the user.
//!
//! Values are stored to two decimal places. Parsing a string with more precision than that
//! rounds half away from zero; multiplication and division still truncate.
//!
//! Multiplication by a scalar works in either operand order. Division does not: only
//! `Currency / scalar` is implemented, since `scalar / Currency` has no meaningful value.

use std::sync::OnceLock;

use ahash::HashSet;
use iso_currency::IntoEnumIterator;

#[cfg(test)]
extern crate serde_json;

#[cfg(test)]
#[macro_use]
extern crate serde_derive;

use std::{error, fmt, ops, str};

use num::Zero;
use num::bigint::{BigInt, BigUint, Sign};
use num::traits::FromPrimitive;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const DECIMAL_PLACES: usize = 2;
const SECTION_LEN: usize = 3; // 1,323.00 <- "323" is a section

/// Represents currency through an optional symbol and amount of coin.
///
/// Every 100 coins represents a banknote. (coin: 100 => 1.00)
#[derive(Debug, Clone, Hash, Default, PartialEq, Eq, PartialOrd)]
pub struct Currency {
    symbol: String,
    coin: BigInt,
}

impl Currency {
    /// Creates a blank Currency with no symbol and 0 coin.
    #[must_use]
    pub fn new() -> Self {
        Currency {
            symbol: String::new(),
            coin: BigInt::zero(),
        }
    }

    /// Creates a `Currency` from the specified values.
    ///
    /// # Examples
    ///
    /// ```
    /// use qsv_currency::Currency;
    ///
    /// let c = Currency::from(1000, '$');
    /// assert_eq!(c, Currency::from_str("$10.00").unwrap());
    /// ```
    pub fn from(coin: impl Into<BigInt>, symbol: impl ToString) -> Currency {
        Currency {
            symbol: symbol.to_string(),
            coin: coin.into(),
        }
    }

    /// Parses a string literal (&str) and attempts to convert it into a currency. Returns
    /// `Ok(Currency)` on a successful conversion, otherwise `Err(ParseCurrencyError)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use qsv_currency::Currency;
    ///
    /// let c1 = Currency::from_str("$42.32").unwrap();
    /// let c2 = Currency::from_str("$0.10").unwrap();
    /// assert_eq!(c1 + c2, Currency::from_str("$42.42").unwrap());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `ParseCurrencyError` only when a character in digit position cannot be parsed
    /// as an unsigned integer, e.g. `"12-34"`. Note that this is a much narrower condition
    /// than "the input was not a currency": parsing is deliberately permissive, so input with
    /// no digits at all succeeds, yielding a zero amount whose symbol is the whole string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Currency, ParseCurrencyError> {
        use num::traits::Signed;
        use std::str::FromStr;

        const fn is_symbol(c: char) -> bool {
            !c.is_ascii_digit() && c != '-' && c != '.' && c != ',' && c != ')'
        }

        const fn is_delimiter(c: char) -> bool {
            c == '.' || c == ','
        }

        let err = ParseCurrencyError::new(s);

        let mut digits = String::new();
        let mut symbol = String::new();
        let mut sign = Sign::Plus;

        let mut symbol_ended = false;

        let mut last_delimiter = None;
        let mut last_streak_len = 0;
        for c in s.chars() {
            if (c == '(' || c == '-') && digits.is_empty() {
                if !symbol.is_empty() {
                    symbol_ended = true;
                }
                sign = Sign::Minus;
            } else if is_delimiter(c) {
                if !symbol.is_empty() {
                    symbol_ended = true;
                }
                last_streak_len = 0;
                last_delimiter = Some(c);
            } else if is_symbol(c) {
                if !symbol_ended {
                    symbol.push(c);
                }
            } else if c == ')' {
                break;
            } else {
                symbol_ended = true;
                last_streak_len += 1;
                digits.push(c);
            }
        }

        let unsigned_bigint = if digits.is_empty() {
            BigUint::zero()
        } else {
            // note: no println! here — this is a library, and for qsv stdout is the
            // data channel, so writing to it can corrupt piped output
            let Ok(int) = BigUint::from_str(&digits) else {
                return Err(err);
            };
            int
        };
        let mut coin = BigInt::from_biguint(sign, unsigned_bigint);

        // decimal adjustment
        if last_delimiter.is_none() || last_streak_len == 3 {
            // no decimal at all
            coin *= BigInt::from(100);
        } else if last_streak_len < 2 {
            // specifying less cents than needed
            coin *= BigInt::from(10).pow(2 - last_streak_len);
        } else if last_streak_len > 2 {
            // More cents than we can hold, so round half away from zero.
            //
            // This is done entirely in BigInt rather than by way of f64: going through a
            // float used to lose precision on large values, overflow the exponent past 11
            // decimal places, and — because the cast to u64 saturates — silently turn every
            // negative value into zero.
            let divisor = BigInt::from(10).pow(last_streak_len - 2);
            let magnitude = coin.abs();
            let quotient = &magnitude / &divisor;
            let remainder = &magnitude % &divisor;
            let rounded = if remainder * 2u8 >= divisor {
                quotient + 1u8
            } else {
                quotient
            };
            coin = if sign == Sign::Minus {
                -rounded
            } else {
                rounded
            };
        } // else the user has valid cents, no adjustment needed

        let currency = Currency {
            // trim both ends: leading whitespace is common in CSV columns, and leaving
            // it in the symbol breaks is_iso_currency() and panics on arithmetic
            symbol: symbol.trim().to_string(),
            coin,
        };

        Ok(currency)
    }

    /// Returns the `Sign` of the `BigInt` holding the coins.
    #[must_use]
    #[inline]
    pub fn sign(&self) -> Sign {
        self.coin.sign()
    }

    /// Returns the number of coins held in the `Currency` as `&BigInt`.
    ///
    /// Should you need ownership of the returned `BigInt`, call `clone()` on it.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate num;
    /// extern crate qsv_currency;
    ///
    /// fn main() {
    ///     use num::traits::ToPrimitive;
    ///     use qsv_currency::Currency;
    ///
    ///     let c1 = Currency::new();
    ///     assert_eq!(c1.value().to_u32().unwrap(), 0);
    ///
    ///     let c2 = Currency::from_str("$1.42").unwrap();
    ///     assert_eq!(c2.value().to_u32().unwrap(), 142);
    /// }
    /// ```
    #[must_use]
    pub const fn value(&self) -> &BigInt {
        &self.coin
    }

    /// Returns the symbol of the `Currency` as `&str`.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate qsv_currency;
    ///
    /// fn main() {
    ///     use qsv_currency::Currency;
    ///
    ///     let c1 = Currency::from_str("USD1.00").unwrap();
    ///     assert_eq!(c1.symbol(), "USD");
    ///     
    ///     let c2 = Currency::from_str("€1.00").unwrap();
    ///     assert_eq!(c2.symbol(), "€");
    ///
    ///     let c3 = Currency::from_str("1.00").unwrap();
    ///     assert_eq!(c3.symbol(), "");
    /// }
    /// ```
    #[must_use]
    #[inline]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns true if the currency is an ISO currency or a valid ISO currency symbol.
    ///
    /// # Examples
    ///
    /// ```
    /// use qsv_currency::Currency;
    ///
    /// let currency = Currency::from_str("USD 100").unwrap();
    /// assert!(currency.is_iso_currency());
    ///
    /// let currency = Currency::from_str("$ 100").unwrap();
    /// assert!(currency.is_iso_currency());
    ///
    /// let currency = Currency::from_str("¥ 100").unwrap();
    /// assert!(currency.is_iso_currency());
    ///
    /// let currency = Currency::from_str("JPY 1000.00").unwrap();
    /// assert!(currency.is_iso_currency());
    ///
    /// // ISO currency is case sensitive
    /// let currency = Currency::from_str("USd 100").unwrap();
    /// assert!(!currency.is_iso_currency());
    ///
    /// // crypto currency like DOGE, Ethereum, Bitcoin, etc. are not ISO currencies
    /// let currency = Currency::from_str("Ð 100").unwrap();
    /// assert!(!currency.is_iso_currency());
    ///
    /// let currency = Currency::from_str("Ξ 100").unwrap();
    /// assert!(!currency.is_iso_currency());
    ///
    /// let currency = Currency::from_str("100").unwrap();
    /// assert!(!currency.is_iso_currency());
    /// ```
    #[inline]
    pub fn is_iso_currency(&self) -> bool {
        // Initialize OnceLock for symbols_map
        static SYMBOLS_MAP: OnceLock<HashSet<String>> = OnceLock::new();

        // Populate symbols_map only once
        let symbols_map = SYMBOLS_MAP.get_or_init(|| {
            let currencies_iter = iso_currency::Currency::iter();
            currencies_iter.map(|c| c.symbol().to_string()).collect()
        });

        iso_currency::Currency::from_code(self.symbol()).is_some()
            || (!self.symbol().is_empty() && symbols_map.contains(self.symbol()))
    }

    /// Sets the symbol of the `Currency`.
    ///
    /// # Examples
    ///
    /// ```
    /// extern crate qsv_currency;
    ///
    /// fn main() {
    ///     use qsv_currency::Currency;
    ///
    ///     let mut c = Currency::from_str("USD1.00").unwrap();
    ///     c.set_symbol('$');
    ///     assert_eq!(c.symbol(), "$");
    ///     assert_eq!(c, Currency::from_str("$1.00").unwrap());
    /// }
    /// ```
    #[inline]
    pub fn set_symbol(&mut self, symbol: impl ToString) {
        self.symbol = symbol.to_string();
    }

    /// Returns a new `Currency` by multiplying the coin by the conversion rate and changing the
    /// symbol.
    ///
    /// # Examples
    ///
    /// ```
    /// use qsv_currency::Currency;
    ///
    /// let dollars = Currency::from_str("$10.00").unwrap();
    /// let conv_rate = 0.89;
    /// let euros = dollars.convert(0.89, '€');
    /// assert_eq!(euros, Currency::from_str("€8.90").unwrap());
    /// ```
    #[must_use]
    pub fn convert(&self, conversion_rate: f64, currency_symbol: impl ToString) -> Currency {
        let mut result = self * conversion_rate;
        result.symbol = currency_symbol.to_string();
        result
    }

    // TODO
    // - to_str with comma delimiting
    // - to_str with euro delimiting
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// fmt trait implementations
///////////////////////////////////////////////////////////////////////////////////////////////////

/// Allows any Currency to be displayed as a String. The format includes comma delimiting with a
/// two digit precision decimal.
///
/// # Example
///
/// ```
/// use qsv_currency::Currency;
///
/// let dollars = Currency::from_str("$12.10").unwrap();
/// assert_eq!(dollars.to_string(), "$12.10");
///
/// let euros = Currency::from_str("£1.000").unwrap();
/// assert_eq!(format!("{:e}", euros), "£1.000,00");
/// ```
impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use num::traits::Signed;

        let mut result = String::new();

        if self.coin.sign() == Sign::Minus {
            result.push('-');
        }

        result.push_str(&self.symbol);

        let digit_str = self.coin.abs().to_str_radix(10);

        // put symbol before first digit
        let n_digits = digit_str.len();
        if n_digits <= DECIMAL_PLACES {
            // gotta put 0.xx or 0.0x
            result.push_str("0.");
            if n_digits == 1 {
                result.push('0');
            }
            result.push_str(&digit_str);
        } else {
            let n_before_dec = n_digits - DECIMAL_PLACES;
            let int_digit_str = &digit_str[0..n_before_dec];
            let dec_digit_str = &digit_str[n_before_dec..n_digits];

            let first_section_len = n_before_dec % SECTION_LEN;
            let mut counter = if first_section_len == 0 {
                0
            } else {
                SECTION_LEN - first_section_len
            };

            for digit in int_digit_str.chars() {
                if counter == SECTION_LEN {
                    counter = 0;
                    result.push(',');
                }
                result.push(digit);
                counter += 1;
            }
            result.push('.');
            result.push_str(dec_digit_str);
        }

        write!(f, "{result}")
    }
}

impl str::FromStr for Currency {
    type Err = ParseCurrencyError;

    fn from_str(s: &str) -> Result<Currency, ParseCurrencyError> {
        Currency::from_str(s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseCurrencyError {
    source: String,
}

impl ParseCurrencyError {
    fn new(s: &str) -> Self {
        ParseCurrencyError {
            source: s.to_string(),
        }
    }
}

impl fmt::Display for ParseCurrencyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Could not parse {} into a currency.", self.source)
    }
}

// `Error::description` is deprecated since Rust 1.42; `Display` carries the message.
impl error::Error for ParseCurrencyError {}

/// Identical to the implementation of Display, but replaces the "." with a ",". Access this
/// formatting by using "{:e}".
///
/// # Example
///
/// ```
/// use qsv_currency::Currency;
///
/// let euros = Currency::from_str("£1000,99").unwrap();
/// println!("{:e}", euros);
/// ```
/// Which prints:
/// ```text
/// "£1.000,99"
/// ```
impl fmt::LowerExp for Currency {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Swap in a single pass. The original implementation round-tripped through an
        // 'x' placeholder, which corrupted any symbol containing an 'x' — "Mex$1.000,99"
        // came out as "Me,$1.000,99".
        let swapped: String = format!("{self}")
            .chars()
            .map(|c| match c {
                '.' => ',',
                ',' => '.',
                other => other,
            })
            .collect();
        write!(f, "{swapped}")
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// ops trait implementations
// macros based on bigint: http://rust-num.github.io/num/src/num_bigint/bigint/src/lib.rs.html
///////////////////////////////////////////////////////////////////////////////////////////////////

macro_rules! impl_all_trait_combinations_for_currency {
    ($module:ident::$imp:ident, $method:ident) => {
        impl<'a, 'b> $module::$imp<&'b Currency> for &'a Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'b Currency) -> Currency {
                if self.symbol == other.symbol {
                    Currency {
                        symbol: self.symbol.clone(),
                        coin: self.coin.clone().$method(other.coin.clone()),
                    }
                } else {
                    panic!("Cannot do arithmetic on two different types of currency.");
                }
            }
        }

        impl<'a> $module::$imp<Currency> for &'a Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                if self.symbol == other.symbol {
                    Currency {
                        symbol: self.symbol.clone(),
                        coin: self.coin.clone().$method(other.coin),
                    }
                } else {
                    panic!("Cannot do arithmetic on two different types of currency.");
                }
            }
        }

        impl<'a> $module::$imp<&'a Currency> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'a Currency) -> Currency {
                if self.symbol == other.symbol {
                    Currency {
                        symbol: self.symbol,
                        coin: self.coin.$method(other.coin.clone()),
                    }
                } else {
                    panic!("Cannot do arithmetic on two different types of currency.");
                }
            }
        }

        impl $module::$imp<Currency> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                if self.symbol == other.symbol {
                    Currency {
                        symbol: self.symbol,
                        coin: self.coin.$method(other.coin),
                    }
                } else {
                    panic!("Cannot do arithmetic on two different types of currency.");
                }
            }
        }
    };
}

impl_all_trait_combinations_for_currency!(ops::Add, add);
impl_all_trait_combinations_for_currency!(ops::Sub, sub);
// impl_all_trait_combinations_for_currency!(ops::Mul, mul); TODO decide whether this should exist

// Scalar arithmetic is generated in two halves:
//
//   *_currency_lhs!  -> Currency OP scalar
//   *_scalar_lhs!    -> scalar OP Currency
//
// Multiplication is commutative, so it gets both halves. Division is NOT, so it gets
// only the currency_lhs half: `2.0 / $10.00` has no meaningful Currency value, and the
// generated impls used to answer it by silently computing `$10.00 / 2.0`.

// other type must implement Into<BigInt>
macro_rules! impl_currency_lhs_into_bigint {
    ($module:ident::$imp:ident, $method:ident, $other:ty) => {
        impl<'b> $module::$imp<&'b $other> for &Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'b $other) -> Currency {
                let big_int: BigInt = other.clone().into();
                Currency {
                    symbol: self.symbol.clone(),
                    coin: self.coin.clone().$method(big_int),
                }
            }
        }

        impl $module::$imp<$other> for &Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: $other) -> Currency {
                let big_int: BigInt = other.into();
                Currency {
                    symbol: self.symbol.clone(),
                    coin: self.coin.clone().$method(big_int),
                }
            }
        }

        impl<'a> $module::$imp<&'a $other> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'a $other) -> Currency {
                let big_int: BigInt = other.clone().into();
                Currency {
                    symbol: self.symbol,
                    coin: self.coin.$method(big_int),
                }
            }
        }

        impl $module::$imp<$other> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: $other) -> Currency {
                let big_int: BigInt = other.into();
                Currency {
                    symbol: self.symbol,
                    coin: self.coin.$method(big_int),
                }
            }
        }
    };
}

macro_rules! impl_scalar_lhs_into_bigint {
    ($module:ident::$imp:ident, $method:ident, $other:ty) => {
        impl<'b> $module::$imp<&'b Currency> for &$other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'b Currency) -> Currency {
                let big_int: BigInt = self.clone().into();
                Currency {
                    symbol: other.symbol.clone(),
                    coin: other.coin.clone().$method(big_int),
                }
            }
        }

        impl $module::$imp<Currency> for &$other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                let big_int: BigInt = self.clone().into();
                Currency {
                    symbol: other.symbol,
                    coin: other.coin.$method(big_int),
                }
            }
        }

        impl<'a> $module::$imp<&'a Currency> for $other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'a Currency) -> Currency {
                let big_int: BigInt = self.into();
                Currency {
                    symbol: other.symbol.clone(),
                    coin: other.coin.clone().$method(big_int),
                }
            }
        }

        impl $module::$imp<Currency> for $other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                let big_int: BigInt = self.into();
                Currency {
                    symbol: other.symbol,
                    coin: other.coin.$method(big_int),
                }
            }
        }
    };
}

macro_rules! impl_int_scalar_ops {
    ($($other:ty),* $(,)?) => {
        $(
            // multiplication is commutative, so both operand orders are generated
            impl_currency_lhs_into_bigint!(ops::Mul, mul, $other);
            impl_scalar_lhs_into_bigint!(ops::Mul, mul, $other);
            // division is not: only `Currency / scalar` is meaningful
            impl_currency_lhs_into_bigint!(ops::Div, div, $other);
        )*
    };
}

impl_int_scalar_ops!(BigUint, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

///////////////////////////////////////////////////////////////////////////////////////////////////
// float scalar arithmetic

/// Converts a float into the equivalent number of coins (i.e. scaled by 100).
///
/// # Panics
/// Panics if `value` is NaN or infinite. The `Mul`/`Div` operators cannot return a
/// `Result`, so a non-finite operand is a caller bug rather than a recoverable error.
macro_rules! define_scaled_from_float {
    ($name:ident, $float:ty, $conv_method:ident) => {
        #[inline]
        fn $name(value: $float) -> BigInt {
            BigInt::$conv_method(value * 100.0).unwrap_or_else(|| {
                panic!("cannot use a non-finite value ({value}) as a currency operand")
            })
        }
    };
}

define_scaled_from_float!(scaled_from_f32, f32, from_f32);
define_scaled_from_float!(scaled_from_f64, f64, from_f64);

// The two formulas below are the whole reason the float ops are split from the integer
// ops: `scaled` carries a factor of 100 that has to be cancelled on the correct side.
// Each formula is written exactly once.

/// `coin * scalar`, undoing the 100 that `scaled` carries.
#[inline]
fn combine_mul(coin: BigInt, scaled: BigInt) -> BigInt {
    coin * scaled / BigInt::from(100)
}

/// `coin / scalar`. The 100 scales the *numerator*; dividing the result by 100 instead
/// (as this once did) is wrong by a factor of 10,000.
#[inline]
fn combine_div(coin: BigInt, scaled: BigInt) -> BigInt {
    coin * BigInt::from(100) / scaled
}

macro_rules! impl_currency_lhs_float {
    ($module:ident::$imp:ident, $method:ident, $other:ty, $scaled:ident, $combine:ident) => {
        impl<'b> $module::$imp<&'b $other> for &Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'b $other) -> Currency {
                Currency {
                    symbol: self.symbol.clone(),
                    coin: $combine(self.coin.clone(), $scaled(*other)),
                }
            }
        }

        impl $module::$imp<$other> for &Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: $other) -> Currency {
                Currency {
                    symbol: self.symbol.clone(),
                    coin: $combine(self.coin.clone(), $scaled(other)),
                }
            }
        }

        impl<'a> $module::$imp<&'a $other> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'a $other) -> Currency {
                Currency {
                    symbol: self.symbol,
                    coin: $combine(self.coin, $scaled(*other)),
                }
            }
        }

        impl $module::$imp<$other> for Currency {
            type Output = Currency;

            #[inline]
            fn $method(self, other: $other) -> Currency {
                Currency {
                    symbol: self.symbol,
                    coin: $combine(self.coin, $scaled(other)),
                }
            }
        }
    };
}

macro_rules! impl_scalar_lhs_float {
    ($module:ident::$imp:ident, $method:ident, $other:ty, $scaled:ident, $combine:ident) => {
        impl<'b> $module::$imp<&'b Currency> for &$other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'b Currency) -> Currency {
                Currency {
                    symbol: other.symbol.clone(),
                    coin: $combine(other.coin.clone(), $scaled(*self)),
                }
            }
        }

        impl $module::$imp<Currency> for &$other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                Currency {
                    symbol: other.symbol,
                    coin: $combine(other.coin, $scaled(*self)),
                }
            }
        }

        impl<'a> $module::$imp<&'a Currency> for $other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: &'a Currency) -> Currency {
                Currency {
                    symbol: other.symbol.clone(),
                    coin: $combine(other.coin.clone(), $scaled(self)),
                }
            }
        }

        impl $module::$imp<Currency> for $other {
            type Output = Currency;

            #[inline]
            fn $method(self, other: Currency) -> Currency {
                Currency {
                    symbol: other.symbol,
                    coin: $combine(other.coin, $scaled(self)),
                }
            }
        }
    };
}

// multiplication by a float is commutative
impl_currency_lhs_float!(ops::Mul, mul, f32, scaled_from_f32, combine_mul);
impl_currency_lhs_float!(ops::Mul, mul, f64, scaled_from_f64, combine_mul);
impl_scalar_lhs_float!(ops::Mul, mul, f32, scaled_from_f32, combine_mul);
impl_scalar_lhs_float!(ops::Mul, mul, f64, scaled_from_f64, combine_mul);

// division is not — `2.0 / $10.00` is deliberately not implemented
impl_currency_lhs_float!(ops::Div, div, f32, scaled_from_f32, combine_div);
impl_currency_lhs_float!(ops::Div, div, f64, scaled_from_f64, combine_div);

/// Overloads the '/' operator between two borrowed Currency objects.
///
/// # Panics
/// Panics if they aren't the same type of currency, as denoted by the currency's symbol.
impl<'b> ops::Div<&'b Currency> for &Currency {
    type Output = BigInt;

    fn div(self, other: &'b Currency) -> BigInt {
        if self.symbol == other.symbol {
            self.coin.clone() / other.coin.clone()
        } else {
            panic!("Cannot divide two different types of currency.");
        }
    }
}

/// Overloads the '/' operator between a borrowed Currency object and an owned one.
///
/// # Panics
/// Panics if they aren't the same type of currency, as denoted by the currency's symbol.
impl ops::Div<Currency> for &Currency {
    type Output = BigInt;

    fn div(self, other: Currency) -> BigInt {
        if self.symbol == other.symbol {
            self.coin.clone() / other.coin
        } else {
            panic!("Cannot divide two different types of currency.");
        }
    }
}

/// Overloads the '/' operator between an owned Currency object and a borrowed one.
///
/// # Panics
/// Panics if they aren't the same type of currency, as denoted by the currency's symbol.
impl<'a> ops::Div<&'a Currency> for Currency {
    type Output = BigInt;

    fn div(self, other: &'a Currency) -> BigInt {
        if self.symbol == other.symbol {
            self.coin / other.coin.clone()
        } else {
            panic!("Cannot divide two different types of currency.");
        }
    }
}

/// Overloads the '/' operator between two owned Currency objects.
///
/// # Panics
/// Panics if they aren't the same type of currency, as denoted by the currency's symbol.
impl ops::Div<Currency> for Currency {
    type Output = BigInt;

    fn div(self, other: Currency) -> BigInt {
        if self.symbol == other.symbol {
            self.coin / other.coin
        } else {
            panic!("Cannot divide two different types of currency.");
        }
    }
}

impl ops::Neg for Currency {
    type Output = Currency;

    fn neg(self) -> Currency {
        Currency {
            symbol: self.symbol,
            coin: -self.coin,
        }
    }
}

impl ops::Neg for &Currency {
    type Output = Currency;

    fn neg(self) -> Currency {
        Currency {
            symbol: self.symbol.clone(),
            coin: -self.coin.clone(),
        }
    }
}

// TODO
// - rem
// - signed

/// Deserializes from a JSON string (not a number), e.g. `{"amount": "-$12,000.99"}`.
///
/// Note that this inherits `from_str`'s permissiveness: a string with no digits in it
/// deserializes to a zero amount rather than failing, so `"garbage"` yields `$0.00` with
/// the symbol set to `"garbage"`.
impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Currency::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Currency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Currency;
    use num::bigint::BigInt;

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "a flat list of parse assertions; splitting it would only scatter them"
    )]
    fn test_from_str() {
        // rounding
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1001),
        };
        let actual = Currency::from_str("$10.0099").unwrap();
        assert_eq!(expected, actual);
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(10078),
        };
        let actual = Currency::from_str("$100.777777").unwrap();
        assert_eq!(expected, actual);

        // TODO rounding still doesn't work when you have three decimal places
        // let expected = Currency { symbol: "$".into(), coin: BigInt::from(10078) };
        // let actual = Currency::from_str("$100.777").unwrap();
        // assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1210),
        };
        let actual = Currency::from_str("$12.10").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$12.100000").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$12.1").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: String::new(),
            coin: BigInt::from(1210),
        };
        let actual = Currency::from_str("12.10").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("12.100000").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("12.1").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: String::new(),
            coin: BigInt::from(-1210),
        };
        let actual = Currency::from_str("(12.10)").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(121_000),
        };
        let actual = Currency::from_str("$1210").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$1,210").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$1,210.00").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$1210.").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$1,210.0").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$1.210,0").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1_200_099),
        };
        let actual = Currency::from_str("$12,000.99").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "£".into(),
            coin: BigInt::from(1_200_099),
        };
        let actual = Currency::from_str("£12,000.99").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(-1_200_099),
        };
        let actual = Currency::from_str("-$12,000.99").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("($12,000.99)").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$(12,000.99)").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(-1210),
        };
        let actual = Currency::from_str("-$12.10").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("$-12.10").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("($12.10)").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("($12.1)").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "€".into(),
            coin: BigInt::from(-12000),
        };
        let actual = Currency::from_str("-€120.00").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("-€120").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("-€-120.0").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("-€120").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("(€120)").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "USD".into(),
            coin: BigInt::from(-12000),
        };
        let actual = Currency::from_str("-USD120").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("USD-120").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("(USD1€20EUR)JPY").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("USD(D120)").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: String::new(),
            coin: BigInt::from(12000),
        };
        let actual = Currency::from_str("120USD").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("1U2S0D").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "€".into(),
            coin: BigInt::from(0),
        };
        let actual = Currency::from_str("€0").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€00.00").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€.00000000").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€0.0").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€000,000.00").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€000,000").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€").unwrap();
        assert_eq!(expected, actual);
        let actual = Currency::from_str("€)10.99asdf").unwrap();
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1000),
        };
        let actual = Currency::from_str("$10.0001").unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_eq() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1210),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1210),
        };
        let c = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1251),
        };

        assert_eq!(a, b);
        assert_eq!(b, b);
        assert_eq!(b, a);
        assert_ne!(a, c);
    }

    #[test]
    fn test_ord() {
        use std::cmp::Ordering;

        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1210),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let c = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1311),
        };
        let d = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1210),
        };

        assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
        assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
        assert_eq!(a.partial_cmp(&d), Some(Ordering::Equal));
        assert_eq!(c.partial_cmp(&a), Some(Ordering::Greater));

        assert!(a < b);
        assert!(a < c);
        assert!(a <= a);
        assert!(a <= c);
        assert!(b > a);
        assert!(c > a);
        assert!(a >= a);
        assert!(c >= a);
    }

    #[test]
    fn test_add() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1311),
        };
        let expected_sum = Currency {
            symbol: "$".into(),
            coin: BigInt::from(2522),
        };
        let actual_sum = a + b;
        assert_eq!(expected_sum, actual_sum);
    }

    #[test]
    fn test_add_commutative() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1311),
        };
        assert_eq!(&a + &b, &b + &a);
    }

    #[test]
    fn test_sub() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1311),
        };

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(-100),
        };
        let actual = &a - &b;
        assert_eq!(expected, actual);

        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(100),
        };
        let actual = b - a;
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_mul() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let f = 0.97;
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1174),
        };
        let actual = a * f;
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_mul_commutative() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(1211),
        };
        let f = 0.97;
        assert_eq!(&a * f, f * &a);
    }

    #[test]
    fn test_div() {
        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(2500),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(500),
        };
        let expected = BigInt::from(5);
        let actual = a / b;
        assert_eq!(expected, actual);

        let a = Currency {
            symbol: "$".into(),
            coin: BigInt::from(3248),
        };
        let b = Currency {
            symbol: "$".into(),
            coin: BigInt::from(888),
        };
        let expected = BigInt::from(3);
        let actual = a / b;
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_neg() {
        let c = Currency {
            symbol: "$".into(),
            coin: BigInt::from(3248),
        };
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(-3248),
        };
        let actual = -c;
        assert_eq!(expected, actual);

        let c = Currency {
            symbol: "$".into(),
            coin: BigInt::from(-3248),
        };
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(3248),
        };
        let actual = -c;
        assert_eq!(expected, actual);

        let c = Currency {
            symbol: "$".into(),
            coin: BigInt::from(0),
        };
        let expected = Currency {
            symbol: "$".into(),
            coin: BigInt::from(0),
        };
        let actual = -c;
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_convert() {
        let dollars = Currency::from_str("$12.50").unwrap();
        let euro_conversion_rate = 0.89;
        let euros = dollars.convert(euro_conversion_rate, '€');
        let expected = Currency {
            symbol: '€'.to_string(),
            coin: BigInt::from(1112),
        };
        assert_eq!(expected, euros);
    }

    #[test]
    fn test_display() {
        use num::traits::Num;

        assert_eq!(
            Currency {
                symbol: "$".into(),
                coin: BigInt::from(0)
            }
            .to_string(),
            "$0.00"
        );

        assert_eq!(
            Currency {
                symbol: "$".into(),
                coin: BigInt::from(-1)
            }
            .to_string(),
            "-$0.01"
        );

        assert_eq!(
            Currency {
                symbol: String::new(),
                coin: BigInt::from(11)
            }
            .to_string(),
            "0.11"
        );

        assert_eq!(
            Currency {
                symbol: String::new(),
                coin: BigInt::from(1210)
            }
            .to_string(),
            "12.10"
        );

        assert_eq!(
            Currency {
                symbol: "$".into(),
                coin: BigInt::from(1210)
            }
            .to_string(),
            "$12.10"
        );

        assert_eq!(
            Currency {
                symbol: String::new(),
                coin: BigInt::from(10000)
            }
            .to_string(),
            "100.00"
        );

        assert_eq!(
            Currency {
                symbol: "£".into(),
                coin: BigInt::from(100_010)
            }
            .to_string(),
            "£1,000.10"
        );

        assert_eq!(
            Currency {
                symbol: "USD".into(),
                coin: BigInt::from(100_010)
            }
            .to_string(),
            "USD1,000.10"
        );

        assert_eq!(
            Currency {
                symbol: "USD ".into(),
                coin: BigInt::from(100_010)
            }
            .to_string(),
            "USD 1,000.10"
        );

        assert_eq!(
            Currency {
                symbol: "$".into(),
                coin: BigInt::from_str_radix("123456789001", 10).unwrap()
            }
            .to_string(),
            "$1,234,567,890.01"
        );

        assert_eq!(
            Currency {
                symbol: "$".into(),
                coin: BigInt::from_str_radix("-123456789001", 10).unwrap()
            }
            .to_string(),
            "-$1,234,567,890.01"
        );
    }

    #[test]
    fn test_foreign_display() {
        assert_eq!(
            format!(
                "{:e}",
                Currency {
                    symbol: "£".into(),
                    coin: BigInt::from(100_000)
                }
            ),
            "£1.000,00"
        );

        assert_eq!(
            format!(
                "{:e}",
                Currency {
                    symbol: "£".into(),
                    coin: BigInt::from(123_400_101)
                }
            ),
            "£1.234.001,01"
        );
    }

    #[test]
    fn test_deserialize() {
        #[derive(PartialEq, Debug, Deserialize)]
        struct HoldsCurrency {
            amount: Currency,
        }

        let expected = HoldsCurrency {
            amount: Currency::from_str("-$12,000.99").unwrap(),
        };
        let actual: HoldsCurrency =
            ::serde_json::from_str("{\"amount\": \"-$12,000.99\"}").unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_serialize() {
        #[derive(Serialize)]
        struct HoldsCurrency {
            amount: Currency,
        }

        let data = HoldsCurrency {
            amount: Currency {
                symbol: "£".into(),
                coin: BigInt::from(-123_400_101),
            },
        };
        let expected = String::from("{\"amount\":\"-£1,234,001.01\"}");
        let actual = serde_json::to_string(&data).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_iso_currency() {
        // Test valid ISO currencies
        let currency = Currency::from_str("USD1,000,000.00").unwrap();
        assert!(currency.is_iso_currency());

        let currency = Currency::from_str("EUR 100.00").unwrap();
        assert!(currency.is_iso_currency());

        let currency = Currency::from_str("JPY 10000").unwrap();
        assert!(currency.is_iso_currency());

        let currency = Currency::from_str("GBP 50.50").unwrap();
        assert!(currency.is_iso_currency());

        // Test invalid or non-ISO currencies
        let currency = Currency::from_str("FAKE 1000").unwrap();
        assert!(!currency.is_iso_currency());

        let currency = Currency::from_str("BTC 100").unwrap();
        assert!(!currency.is_iso_currency());

        // Test valid ISO currency symbols
        let currency = Currency::from_str("$1,000,000.00").unwrap();
        assert!(currency.is_iso_currency());

        let currency = Currency::from_str("€ 100,000,000.00").unwrap();
        assert!(currency.is_iso_currency());

        let currency = Currency::from_str("£50.00").unwrap();
        assert!(currency.is_iso_currency());

        // Test non-ISO currency symbols
        let currency = Currency::from_str("₿ 50.00").unwrap();
        assert!(!currency.is_iso_currency());

        let currency = Currency::from_str("Ð 50.00").unwrap();
        assert!(!currency.is_iso_currency());

        let currency = Currency::from_str("Ξ 1,990.00").unwrap();
        assert!(!currency.is_iso_currency());

        // Test edge cases
        let currency = Currency::from_str("").unwrap();
        assert!(!currency.is_iso_currency());

        let currency = Currency::from_str("123.45").unwrap();
        assert!(!currency.is_iso_currency());

        // Test case sensitivity
        let currency = Currency::from_str("USd 100.00").unwrap();
        assert!(!currency.is_iso_currency());

        let currency = Currency::from_str("USD100.00").unwrap();
        assert!(currency.is_iso_currency());
    }

    // Regression tests for the defects found in the 2026-08 review. Each of these was
    // first written to assert the *wrong* value the code produced at the time, confirmed
    // to pass, then flipped to the correct expectation.
    mod regressions {
        use super::super::Currency;
        use num::bigint::BigInt;

        // Negatives used to saturate to zero: format!("{coin}") is signed, so the f64
        // round-trip produced a negative, and `.round() as u64` clamps that to 0.
        #[test]
        fn negative_with_extra_decimals_rounds_correctly() {
            assert_eq!(
                *Currency::from_str("-$12.9999").unwrap().value(),
                BigInt::from(-1300)
            );
            // half-away-from-zero at the boundary (4 decimals: a 3-digit streak is
            // deliberately read as a thousands separator instead)
            assert_eq!(
                *Currency::from_str("-$0.0050").unwrap().value(),
                BigInt::from(-1)
            );
            // rounding is symmetric about zero
            assert_eq!(
                Currency::from_str("-$12.9999").unwrap().value().magnitude(),
                Currency::from_str("$12.9999").unwrap().value().magnitude()
            );
        }

        // `coin.div(scalar * 100) / 100` was wrong by a factor of 10,000.
        #[test]
        fn division_by_float_is_exact() {
            let ten = Currency::from_str("$10.00").unwrap();
            assert_eq!(*(&ten / 2.0f64).value(), BigInt::from(500));
            assert_eq!(*(&ten / 0.5f64).value(), BigInt::from(2000));
            assert_eq!(*(&ten / 4.0f32).value(), BigInt::from(250));
            // multiplication was already correct; guard against regressing it
            assert_eq!(*(&ten * 0.97f64).value(), BigInt::from(970));
        }

        // Large values used to lose precision through f64 and overflow `10u32.pow` past
        // 11 decimal places.
        #[test]
        fn many_decimals_neither_panic_nor_lose_precision() {
            assert_eq!(
                *Currency::from_str("$1.000000000000").unwrap().value(),
                BigInt::from(100)
            );
            let big = "$123456789012345678901234567890.987654321";
            assert_eq!(
                Currency::from_str(big).unwrap().to_string(),
                "$123,456,789,012,345,678,901,234,567,890.99"
            );
        }

        // `trim_end` left leading whitespace in the symbol, which broke ISO detection and
        // made arithmetic against an unpadded value panic.
        #[test]
        fn leading_whitespace_is_trimmed_from_symbol() {
            let padded = Currency::from_str("  $1.00").unwrap();
            assert_eq!(padded.symbol(), "$");
            assert!(padded.is_iso_currency());
            assert_eq!(padded + Currency::from_str("$1.00").unwrap(), {
                Currency::from_str("$2.00").unwrap()
            });
        }

        #[test]
        #[should_panic(expected = "non-finite")]
        fn non_finite_float_panics_with_a_clear_message() {
            let _ = Currency::from_str("$1.00").unwrap() * f64::NAN;
        }

        // The old `{:e}` swapped ',' and '.' via an 'x' placeholder, mangling any symbol
        // containing an 'x'.
        #[test]
        fn lower_exp_preserves_x_in_symbol() {
            let c = Currency::from_str("Mex$1.000,99").unwrap();
            assert_eq!(c.symbol(), "Mex$");
            assert_eq!(format!("{c:e}"), "Mex$1.000,99");
        }
    }
}
