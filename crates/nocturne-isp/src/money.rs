//! Money: a whole number of minor units with the currency beside it.
//!
//! Specification: `docs/dev/features/isp.md` ("Money"). Everything in this
//! module exists to make one class of bug unrepresentable rather than
//! unlikely.
//!
//! * **No floating point.** There is no constructor, no parser and no
//!   deserialiser here that accepts an `f64`, and [`Money`] has no field one
//!   could be stored in. A JSON number with a decimal point in an amount is
//!   refused with a message that says what to send instead. `0.1 + 0.2` is
//!   not `0.3`, and a rounding error in a ledger is indistinguishable from
//!   theft when somebody audits it.
//! * **The minor unit is per currency.** `USD` has two minor digits and
//!   `IQD` has none, so `1000` is ten dollars or a thousand dinars depending
//!   on the currency that travels with it. A hard-coded `100` anywhere in
//!   this crate is a bug; [`Currency::scale`] is the only source of it.
//! * **An unknown currency is refused on entry.** [`CURRENCIES`] is the whole
//!   list; a code that is not in it is an error, never two digits assumed.
//! * **Arithmetic returns a `Result`.** Adding `USD` to `IQD` is an error
//!   rather than a number, and so is an overflow. That is why the operations
//!   are [`Money::plus`], [`Money::minus`] and [`Money::times`] rather than
//!   `impl Add`: an operator makes a mixed sum too easy to write by accident,
//!   and `+` has nowhere to put the refusal.
//! * **A mixed total is not a number.** [`Totals`] is the only way to add up
//!   amounts of more than one currency, it has no scalar and it serialises as
//!   one figure per currency. Converting one is a separate, explicit step
//!   that carries the rate and the date it used ([`convert_totals`]).

use std::collections::BTreeMap;
use std::fmt;

use nocturne_app::{AppError, AppResult};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

// ------------------------------------------------------------------ currency

/// One currency Nocturne will accept an amount in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrencyInfo {
    pub code: &'static str,
    /// Digits after the point in everyday use — the power of ten between the
    /// minor unit an amount is stored in and the unit people say out loud.
    pub digits: u32,
    pub name: &'static str,
}

impl CurrencyInfo {
    const fn new(code: &'static str, digits: u32, name: &'static str) -> CurrencyInfo {
        CurrencyInfo { code, digits, name }
    }
}

/// Every currency this crate will take an amount in, by ISO 4217 code and in
/// code order, with the number of minor digits actually used.
///
/// `IQD` is **0** here, not the 3 that ISO 4217 still lists: the fils is long
/// out of circulation and Iraqi dinars are quoted, invoiced and paid whole.
/// `docs/dev/features/isp.md` settles that, and it is the reason this table
/// exists at all rather than a call to a library: the digits a ledger uses
/// are a decision about the business, and a wrong one moves every amount in
/// it by a factor of a thousand.
///
/// Currencies whose everyday practice and ISO's minor unit disagree the way
/// the dinar's do (the Lebanese pound, the Iranian rial, the Syrian pound)
/// are deliberately absent rather than guessed at: adding one is a line here
/// and a decision recorded beside it.
pub const CURRENCIES: &[CurrencyInfo] = &[
    CurrencyInfo::new("AED", 2, "UAE dirham"),
    CurrencyInfo::new("AUD", 2, "Australian dollar"),
    CurrencyInfo::new("BHD", 3, "Bahraini dinar"),
    CurrencyInfo::new("CAD", 2, "Canadian dollar"),
    CurrencyInfo::new("CHF", 2, "Swiss franc"),
    CurrencyInfo::new("EGP", 2, "Egyptian pound"),
    CurrencyInfo::new("EUR", 2, "Euro"),
    CurrencyInfo::new("GBP", 2, "Pound sterling"),
    CurrencyInfo::new("INR", 2, "Indian rupee"),
    // Deliberately 0 minor digits: see this table's own note above.
    CurrencyInfo::new("IQD", 0, "Iraqi dinar"),
    CurrencyInfo::new("JOD", 3, "Jordanian dinar"),
    CurrencyInfo::new("JPY", 0, "Japanese yen"),
    CurrencyInfo::new("KES", 2, "Kenyan shilling"),
    CurrencyInfo::new("KRW", 0, "South Korean won"),
    CurrencyInfo::new("KWD", 3, "Kuwaiti dinar"),
    CurrencyInfo::new("OMR", 3, "Omani rial"),
    CurrencyInfo::new("PKR", 2, "Pakistani rupee"),
    CurrencyInfo::new("SAR", 2, "Saudi riyal"),
    CurrencyInfo::new("TRY", 2, "Turkish lira"),
    CurrencyInfo::new("USD", 2, "United States dollar"),
    CurrencyInfo::new("ZAR", 2, "South African rand"),
];

/// A currency from [`CURRENCIES`]. There is no way to make one that is not
/// in that table, so every [`Money`] in the system has known minor digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency {
    code: &'static str,
    digits: u32,
}

impl Currency {
    /// The currency `code` names, case-insensitively.
    pub fn of(code: &str) -> AppResult<Currency> {
        let wanted = code.trim();
        CURRENCIES
            .iter()
            .find(|known| known.code.eq_ignore_ascii_case(wanted))
            .map(|known| Currency {
                code: known.code,
                digits: known.digits,
            })
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "`{wanted}` is not a currency Nocturne knows the minor unit of, and it will \
                     not assume one. Accepted: {}. Adding another is a change to \
                     `nocturne-isp::money::CURRENCIES`, with its minor digits written down.",
                    CURRENCIES
                        .iter()
                        .map(|known| known.code)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    pub fn code(self) -> &'static str {
        self.code
    }

    /// Digits after the point: 2 for `USD`, 0 for `IQD`.
    pub fn digits(self) -> u32 {
        self.digits
    }

    pub fn name(self) -> &'static str {
        CURRENCIES
            .iter()
            .find(|known| known.code == self.code)
            .map(|known| known.name)
            .unwrap_or(self.code)
    }

    /// Minor units in one major unit: 100 for `USD`, **1** for `IQD`. The
    /// only place a power of ten belongs.
    pub fn scale(self) -> i64 {
        // `digits` comes from the table above, where the largest is 3.
        10_i64.pow(self.digits)
    }

    pub fn zero(self) -> Money {
        Money {
            minor: 0,
            currency: self,
        }
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code)
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Currency, D::Error> {
        let code = String::deserialize(deserializer)?;
        Currency::of(&code).map_err(|err| serde::de::Error::custom(err.message))
    }
}

// --------------------------------------------------------------------- money

/// An amount: whole minor units of one currency.
///
/// Serialises as `{currency, minor, digits, text}` — `minor` is the ledger,
/// `digits` and `text` are there so no screen has to know that `1000` is ten
/// dollars or a thousand dinars. It deserialises from `{currency, minor}` or
/// `{currency, text}` (an exact decimal string), and from its own output.
///
/// It is deliberately not `Ord`: `1000 IQD` and `1000 USD` would compare
/// equal, and a sort or a `max` across currencies is the same mistake as a
/// mixed total. Compare `minor` within one currency, as the reports do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Money {
    /// Whole minor units. Never fractional, never floating point.
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Money {
        Money { minor, currency }
    }

    pub fn zero(currency: Currency) -> Money {
        Money { minor: 0, currency }
    }

    pub fn is_zero(self) -> bool {
        self.minor == 0
    }

    pub fn is_negative(self) -> bool {
        self.minor < 0
    }

    /// The amount as people write it: `25.00` for 2500 `USD`, `1000` for 1000
    /// `IQD`. Exact — it is integer division, not a format of a float.
    pub fn text(self) -> String {
        let digits = self.currency.digits as usize;
        if digits == 0 {
            return self.minor.to_string();
        }
        let scale = self.currency.scale();
        let sign = if self.minor < 0 { "-" } else { "" };
        let magnitude = self.minor.unsigned_abs();
        let whole = magnitude / scale.unsigned_abs();
        let fraction = magnitude % scale.unsigned_abs();
        format!("{sign}{whole}.{fraction:0digits$}")
    }

    /// Parse an exact decimal string in `currency`: `25`, `25.5`, `-1.07`.
    ///
    /// More fractional digits than the currency has is an error rather than a
    /// rounding: `25.005 USD` is somebody's mistake, and silently making it
    /// `25.00` or `25.01` is how a ledger stops adding up.
    pub fn parse(text: &str, currency: Currency) -> AppResult<Money> {
        let trimmed = text.trim();
        let (negative, rest) = match trimmed.strip_prefix('-') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, trimmed),
        };
        if rest.is_empty() {
            return Err(AppError::invalid("an amount cannot be empty"));
        }
        let (whole_text, fraction_text) = match rest.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (rest, ""),
        };
        let digits = currency.digits() as usize;
        let bad = |why: &str| {
            AppError::invalid(format!(
                "`{trimmed}` is not an amount in {currency}: {why}. Write it as digits with at \
                 most {digits} decimal place(s), or send the whole number of minor units as \
                 `minor`."
            ))
        };
        if whole_text.is_empty() && fraction_text.is_empty() {
            return Err(bad("there are no digits in it"));
        }
        if !whole_text.chars().all(|c| c.is_ascii_digit())
            || !fraction_text.chars().all(|c| c.is_ascii_digit())
        {
            return Err(bad(
                "only digits, one decimal point and a leading minus sign are allowed (no \
                 thousands separators, no currency symbol)",
            ));
        }
        if fraction_text.len() > digits {
            return Err(bad(&format!(
                "{currency} has {digits} minor digit(s), so {} of them cannot be represented \
                 without inventing a rounding",
                fraction_text.len()
            )));
        }
        let whole: i64 = if whole_text.is_empty() {
            0
        } else {
            whole_text.parse().map_err(|_| bad("it is too large"))?
        };
        // "25.5" in a 2-digit currency is 50 minor units of fraction, not 5.
        let mut fraction: i64 = 0;
        for index in 0..digits {
            let digit = fraction_text
                .as_bytes()
                .get(index)
                .map(|byte| i64::from(byte - b'0'))
                .unwrap_or(0);
            fraction = fraction * 10 + digit;
        }
        let minor = whole
            .checked_mul(currency.scale())
            .and_then(|major| major.checked_add(fraction))
            .ok_or_else(|| bad("it is too large"))?;
        Ok(Money {
            minor: if negative { -minor } else { minor },
            currency,
        })
    }

    fn same_currency(self, other: Money, what: &str) -> AppResult<()> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(AppError::invalid(format!(
                "{} {} and {} {} cannot be {what}: they are different currencies, and nothing \
                 here will convert one into the other without a rate somebody entered",
                self.text(),
                self.currency,
                other.text(),
                other.currency
            )))
        }
    }

    pub fn plus(self, other: Money) -> AppResult<Money> {
        self.same_currency(other, "added")?;
        Ok(Money {
            minor: self
                .minor
                .checked_add(other.minor)
                .ok_or_else(|| overflow("adding"))?,
            currency: self.currency,
        })
    }

    pub fn minus(self, other: Money) -> AppResult<Money> {
        self.same_currency(other, "subtracted")?;
        Ok(Money {
            minor: self
                .minor
                .checked_sub(other.minor)
                .ok_or_else(|| overflow("subtracting"))?,
            currency: self.currency,
        })
    }

    /// Multiply by a whole number — a price per megabit by the megabits.
    pub fn times(self, factor: i64) -> AppResult<Money> {
        Ok(Money {
            minor: self
                .minor
                .checked_mul(factor)
                .ok_or_else(|| overflow("multiplying"))?,
            currency: self.currency,
        })
    }

    /// Multiply by `numerator / denominator` — pro-rating a period, or
    /// turning a yearly price into a monthly one.
    ///
    /// Rounds half away from zero, in whole minor units, and says whether it
    /// had to: a figure that was rounded is still exact in the ledger, but
    /// anybody checking the arithmetic deserves to be told.
    pub fn scale_by(self, numerator: i64, denominator: i64) -> AppResult<Rounded> {
        if denominator == 0 {
            return Err(AppError::invalid(
                "an amount cannot be scaled by a ratio with a zero denominator",
            ));
        }
        let product = i128::from(self.minor) * i128::from(numerator);
        let denominator = i128::from(denominator);
        let (minor, exact) = divide(product, denominator)?;
        Ok(Rounded {
            amount: Money {
                minor,
                currency: self.currency,
            },
            exact,
        })
    }
}

fn overflow(what: &str) -> AppError {
    AppError::invalid(format!(
        "{what} these amounts overflows the ledger's 64-bit minor units; the figure is wrong by \
         several orders of magnitude"
    ))
}

/// Integer division, rounding half away from zero, with the remainder
/// reported rather than hidden.
fn divide(numerator: i128, denominator: i128) -> AppResult<(i64, bool)> {
    let negative = (numerator < 0) != (denominator < 0);
    let numerator_abs = numerator.unsigned_abs();
    let denominator_abs = denominator.unsigned_abs();
    let quotient = numerator_abs / denominator_abs;
    let remainder = numerator_abs % denominator_abs;
    // Half away from zero: 2.5 -> 3, -2.5 -> -3.
    let rounded = if remainder * 2 >= denominator_abs {
        quotient + 1
    } else {
        quotient
    };
    let signed = i128::try_from(rounded).map_err(|_| overflow("scaling"))?;
    let signed = if negative { -signed } else { signed };
    let minor = i64::try_from(signed).map_err(|_| overflow("scaling"))?;
    Ok((minor, remainder == 0))
}

/// An amount that came out of a division, and whether that division was
/// exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rounded {
    pub amount: Money,
    /// `false` when minor units were rounded away.
    pub exact: bool,
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Money", 4)?;
        state.serialize_field("currency", self.currency.code())?;
        state.serialize_field("minor", &self.minor)?;
        state.serialize_field("digits", &self.currency.digits())?;
        state.serialize_field("text", &self.text())?;
        state.end()
    }
}

/// What an amount may look like on the way in. Separate from [`Money`] so
/// that the only way to build one is through a check.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoneyInput {
    currency: String,
    /// Whole minor units. A JSON number with a point in it is refused here.
    #[serde(default)]
    minor: Option<Value>,
    /// An exact decimal string: `"25.00"`.
    #[serde(default)]
    text: Option<String>,
    /// Accepted so that this type round-trips through its own output; it must
    /// agree with the currency.
    #[serde(default)]
    digits: Option<u32>,
}

impl MoneyInput {
    fn into_money(self) -> AppResult<Money> {
        let currency = Currency::of(&self.currency)?;
        if let Some(digits) = self.digits
            && digits != currency.digits()
        {
            return Err(AppError::invalid(format!(
                "{currency} has {} minor digit(s), not {digits}; an amount cannot carry its own \
                 idea of the minor unit",
                currency.digits()
            )));
        }
        let from_minor = match &self.minor {
            None => None,
            Some(Value::Number(number)) => match number.as_i64() {
                Some(minor) if !number.is_f64() => Some(Money::new(minor, currency)),
                _ => {
                    return Err(AppError::invalid(format!(
                        "`{number}` is not an amount: money is a whole number of minor units \
                         ({} per {currency}), never a decimal or floating-point number. Send \
                         `{{\"currency\": \"{currency}\", \"minor\": <whole {currency} minor \
                         units>}}`, or the exact decimal as a string in `text`.",
                        currency.scale()
                    )));
                }
            },
            Some(other) => {
                return Err(AppError::invalid(format!(
                    "`minor` is a whole number of {currency} minor units, not {}",
                    kind_of(other)
                )));
            }
        };
        let from_text = match &self.text {
            Some(text) => Some(Money::parse(text, currency)?),
            None => None,
        };
        match (from_minor, from_text) {
            (Some(minor), Some(text)) if minor != text => Err(AppError::invalid(format!(
                "this amount contradicts itself: `minor` is {} and `text` is `{}`, which is {}",
                minor.minor,
                self.text.unwrap_or_default(),
                text.minor
            ))),
            (Some(amount), _) | (None, Some(amount)) => Ok(amount),
            (None, None) => Err(AppError::invalid(format!(
                "an amount needs `minor` (whole {currency} minor units) or `text` (an exact \
                 decimal string)"
            ))),
        }
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string (decimals go in `text`)",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Money, D::Error> {
        MoneyInput::deserialize(deserializer)?
            .into_money()
            .map_err(|err| serde::de::Error::custom(err.message))
    }
}

// ---------------------------------------------------------------- conversion

/// The rate a payment was actually taken at, recorded on that payment and
/// never looked up again.
///
/// `rate` is minor units of `to` per **one major unit** of `from`, as
/// entered: 1,320 for "1,320 dinars to the dollar" (`from: USD`, `to: IQD`).
/// A rate is kept in the direction it was quoted in, because the other
/// direction does not fit in an integer — 0.00076 USD per dinar rounds to
/// nothing — and [`Conversion::apply`] divides instead of multiplying when
/// the amount is in the `to` currency. There is no rate feed and no default:
/// whoever took the payment enters what they actually got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Conversion {
    pub from: Currency,
    pub to: Currency,
    /// Minor units of `to` per one major unit of `from`, as entered.
    pub rate: i64,
    /// When this rate was taken (ms since the epoch).
    pub at: i64,
}

/// A rate on the way in: `at` is filled in by whatever records it, so that
/// the stored moment is the payment's and not the caller's idea of now.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionInput {
    pub from: String,
    pub to: String,
    pub rate: Value,
    #[serde(default)]
    pub at: Option<i64>,
}

impl ConversionInput {
    pub fn resolve(&self, default_at: i64) -> AppResult<Conversion> {
        let from = Currency::of(&self.from)?;
        let to = Currency::of(&self.to)?;
        if from == to {
            return Err(AppError::invalid(format!(
                "a rate converts between two currencies; `from` and `to` are both {from}"
            )));
        }
        let rate = match &self.rate {
            Value::Number(number) if !number.is_f64() => number.as_i64().unwrap_or(0),
            Value::Number(number) => {
                return Err(AppError::invalid(format!(
                    "`{number}` is not a rate this ledger can keep: a rate is a whole number of \
                     {to} minor units per one {from}, so that the receipt still reconciles in \
                     ten years. Enter what you actually got."
                )));
            }
            other => {
                return Err(AppError::invalid(format!(
                    "a rate is a whole number of {to} minor units per one {from}, not {}",
                    kind_of(other)
                )));
            }
        };
        if rate <= 0 {
            return Err(AppError::invalid(
                "a rate is greater than zero: it is how many minor units of the one currency were \
                 given for one unit of the other",
            ));
        }
        Ok(Conversion {
            from,
            to,
            rate,
            at: self.at.unwrap_or(default_at),
        })
    }
}

/// Read back from a stored payment — and checked again on the way in, so a
/// hand-edited database cannot put a rate of zero or an unknown currency into
/// a receipt. There is no derived `Deserialize`: a rate always arrives through
/// [`ConversionInput`] or this, both of which validate.
impl<'de> Deserialize<'de> for Conversion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Conversion, D::Error> {
        let input = ConversionInput::deserialize(deserializer)?;
        let at = input.at.ok_or_else(|| {
            serde::de::Error::custom(
                "a recorded rate carries the moment it was taken (`at`), because that is what \
                 makes it a fact rather than an opinion",
            )
        })?;
        input
            .resolve(at)
            .map_err(|err| serde::de::Error::custom(err.message))
    }
}

impl Conversion {
    /// What `amount` is worth in the other currency of this pair, at this
    /// rate. An amount in neither currency is an error, never a guess.
    pub fn apply(&self, amount: Money) -> AppResult<Rounded> {
        if amount.currency == self.from {
            // rate is `to` minor per one major of `from`.
            let product = i128::from(amount.minor) * i128::from(self.rate);
            let (minor, exact) = divide(product, i128::from(self.from.scale()))?;
            Ok(Rounded {
                amount: Money::new(minor, self.to),
                exact,
            })
        } else if amount.currency == self.to {
            let product = i128::from(amount.minor) * i128::from(self.from.scale());
            let (minor, exact) = divide(product, i128::from(self.rate))?;
            Ok(Rounded {
                amount: Money::new(minor, self.from),
                exact,
            })
        } else {
            Err(AppError::invalid(format!(
                "this rate is between {} and {}, and the amount is in {}",
                self.from, self.to, amount.currency
            )))
        }
    }

    /// Does this rate relate these two currencies, in either direction?
    pub fn relates(&self, one: Currency, other: Currency) -> bool {
        (self.from == one && self.to == other) || (self.from == other && self.to == one)
    }
}

// -------------------------------------------------------------------- totals

/// A total that spans currencies — which is not a number.
///
/// One figure per currency, in code order. There is no scalar accessor, no
/// `sum()` and no `Deserialize`: a single mixed total is an invented figure,
/// and an invented figure about money is the one somebody will act on. To get
/// one number out of this, a caller must supply the rates and take the answer
/// labelled as a conversion ([`convert_totals`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    by_currency: BTreeMap<&'static str, Money>,
}

impl Totals {
    pub fn new() -> Totals {
        Totals::default()
    }

    /// Start from one amount.
    pub fn of(amount: Money) -> Totals {
        let mut totals = Totals::new();
        // The map is empty, so this cannot be a currency mismatch.
        let _ = totals.add(amount);
        totals
    }

    pub fn add(&mut self, amount: Money) -> AppResult<()> {
        match self.by_currency.get_mut(amount.currency.code()) {
            Some(existing) => {
                *existing = existing.plus(amount)?;
            }
            None => {
                self.by_currency.insert(amount.currency.code(), amount);
            }
        }
        Ok(())
    }

    pub fn subtract(&mut self, amount: Money) -> AppResult<()> {
        self.add(Money::new(
            amount
                .minor
                .checked_neg()
                .ok_or_else(|| overflow("negating"))?,
            amount.currency,
        ))
    }

    pub fn merge(&mut self, other: &Totals) -> AppResult<()> {
        for amount in other.parts() {
            self.add(amount)?;
        }
        Ok(())
    }

    /// The figure in one currency — zero when nothing in it was added, which
    /// is a real zero: nothing was owed in it.
    pub fn in_currency(&self, currency: Currency) -> Money {
        self.by_currency
            .get(currency.code())
            .copied()
            .unwrap_or_else(|| Money::zero(currency))
    }

    /// One amount per currency, in code order.
    pub fn parts(&self) -> Vec<Money> {
        self.by_currency.values().copied().collect()
    }

    pub fn currencies(&self) -> Vec<Currency> {
        self.by_currency
            .values()
            .map(|money| money.currency)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by_currency.is_empty()
    }

    /// True when every figure in here is zero (or there are none).
    pub fn is_zero(&self) -> bool {
        self.by_currency.values().all(|money| money.is_zero())
    }

    /// Drop the currencies whose figure is zero, so a report does not show a
    /// row for a currency nothing happened in.
    pub fn without_zeroes(&self) -> Totals {
        Totals {
            by_currency: self
                .by_currency
                .iter()
                .filter(|(_, money)| !money.is_zero())
                .map(|(code, money)| (*code, *money))
                .collect(),
        }
    }
}

impl Serialize for Totals {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.by_currency.len()))?;
        for amount in self.by_currency.values() {
            sequence.serialize_element(amount)?;
        }
        sequence.end()
    }
}

/// A figure that is **not** the ledger: the result of converting a mixed
/// total at rates a caller supplied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Converted {
    /// Always `"conversion"`. A screen that shows this figure says so.
    pub basis: &'static str,
    pub amount: Money,
    /// The ledger it was computed from, one figure per currency.
    pub of: Totals,
    /// Every rate used, as entered.
    pub rates: Vec<Conversion>,
    /// When the caller says these rates are from.
    pub at: i64,
    /// True when no minor units were rounded away.
    pub exact: bool,
    pub note: String,
}

/// Convert a mixed total into one currency at rates the caller supplies.
///
/// Every currency in `totals` needs a rate (or to be `to` already); a missing
/// one is an error naming the pair, never a currency quietly left out of the
/// answer. The result is labelled a conversion, carries the rates and the
/// date, and says whether it was exact.
pub fn convert_totals(
    totals: &Totals,
    to: Currency,
    rates: &[Conversion],
    at: i64,
) -> AppResult<Converted> {
    let mut sum = Money::zero(to);
    let mut used: Vec<Conversion> = Vec::new();
    let mut exact = true;
    for amount in totals.parts() {
        if amount.currency == to {
            sum = sum.plus(amount)?;
            continue;
        }
        let rate = rates
            .iter()
            .find(|rate| rate.relates(amount.currency, to))
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "there is no rate between {} and {to} in this request, and there is no rate \
                     feed: a converted figure needs the rate you want used, or {} has to be left \
                     out of it",
                    amount.currency, amount.currency
                ))
            })?;
        let converted = rate.apply(amount)?;
        exact = exact && converted.exact;
        sum = sum.plus(converted.amount)?;
        if !used.contains(rate) {
            used.push(*rate);
        }
    }
    Ok(Converted {
        basis: "conversion",
        amount: sum,
        of: totals.clone(),
        rates: used,
        at,
        exact,
        note: format!(
            "a conversion into {to} at the rates given, not the ledger: the amounts owed and \
             received are the per-currency figures in `of`"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usd() -> Currency {
        Currency::of("USD").unwrap()
    }

    fn iqd() -> Currency {
        Currency::of("IQD").unwrap()
    }

    #[test]
    fn a_thousand_minor_units_means_what_the_currency_says_it_means() {
        assert_eq!(usd().digits(), 2);
        assert_eq!(usd().scale(), 100);
        assert_eq!(Money::new(1_000, usd()).text(), "10.00");
        assert_eq!(iqd().digits(), 0, "the fils is out of circulation");
        assert_eq!(iqd().scale(), 1, "a hard-coded 100 would be a bug here");
        assert_eq!(Money::new(1_000, iqd()).text(), "1000");
        // A three-digit currency, so that nothing assumes two.
        let jod = Currency::of("jod").unwrap();
        assert_eq!((jod.digits(), jod.scale()), (3, 1_000));
        assert_eq!(Money::new(1_000, jod).text(), "1.000");
    }

    #[test]
    fn an_unknown_currency_is_refused_rather_than_assumed_to_have_two_digits() {
        let err = Currency::of("XYZ").unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("not a currency"), "{}", err.message);
        assert!(err.message.contains("USD"), "it lists what it accepts");
        assert!(Currency::of("").is_err());
    }

    #[test]
    fn parsing_is_exact_and_refuses_digits_the_currency_cannot_hold() {
        assert_eq!(Money::parse("25", usd()).unwrap().minor, 2_500);
        assert_eq!(Money::parse("25.5", usd()).unwrap().minor, 2_550);
        assert_eq!(Money::parse("25.05", usd()).unwrap().minor, 2_505);
        assert_eq!(Money::parse("-1.07", usd()).unwrap().minor, -107);
        assert_eq!(Money::parse("1000", iqd()).unwrap().minor, 1_000);
        // Not representable, so it is a mistake rather than a rounding.
        let err = Money::parse("25.005", usd()).unwrap_err();
        assert!(err.message.contains("2 minor digit"), "{}", err.message);
        assert!(Money::parse("1000.5", iqd()).is_err(), "dinars are whole");
        assert!(Money::parse("1,000.00", usd()).is_err(), "no separators");
        assert!(Money::parse("$25", usd()).is_err());
        assert!(Money::parse("", usd()).is_err());
        assert!(Money::parse("abc", usd()).is_err());
    }

    #[test]
    fn money_never_deserialises_from_a_floating_point_number() {
        for bad in [json!(25.5), json!(2500.0), json!("2500")] {
            let err = serde_json::from_value::<Money>(json!({"currency": "USD", "minor": bad}))
                .expect_err(&format!("{bad} is not minor units"));
            assert!(
                err.to_string().contains("whole number")
                    || err.to_string().contains("not an amount"),
                "{err}"
            );
        }
        // The two accepted shapes, and its own output.
        let from_minor: Money =
            serde_json::from_value(json!({"currency": "USD", "minor": 2_500})).unwrap();
        let from_text: Money =
            serde_json::from_value(json!({"currency": "USD", "text": "25.00"})).unwrap();
        assert_eq!(from_minor, from_text);
        let round_trip: Money =
            serde_json::from_value(serde_json::to_value(from_minor).unwrap()).unwrap();
        assert_eq!(round_trip, from_minor);
        // Contradicting itself is an error, not a preference for one field.
        assert!(
            serde_json::from_value::<Money>(
                json!({"currency": "USD", "minor": 2_500, "text": "25.01"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<Money>(json!({"currency": "USD"})).is_err());
        assert!(serde_json::from_value::<Money>(json!({"minor": 1})).is_err());
    }

    #[test]
    fn serialised_money_carries_the_minor_unit_so_nothing_has_to_assume_it() {
        let value = serde_json::to_value(Money::new(1_000, iqd())).unwrap();
        assert_eq!(
            value,
            json!({"currency": "IQD", "minor": 1_000, "digits": 0, "text": "1000"})
        );
        assert!(value["minor"].is_i64(), "never a float on the wire");
    }

    #[test]
    fn two_currencies_cannot_be_added_into_one_number() {
        let err = Money::new(2_500, usd())
            .plus(Money::new(1_000, iqd()))
            .unwrap_err();
        assert!(
            err.message.contains("different currencies"),
            "{}",
            err.message
        );
        assert_eq!(
            Money::new(2_500, usd())
                .plus(Money::new(100, usd()))
                .unwrap()
                .minor,
            2_600
        );
        assert!(
            Money::new(i64::MAX, usd())
                .plus(Money::new(1, usd()))
                .is_err()
        );
    }

    #[test]
    fn a_mixed_total_is_one_figure_per_currency_and_has_no_scalar() {
        let mut totals = Totals::new();
        totals.add(Money::new(2_500, usd())).unwrap();
        totals.add(Money::new(1_000, iqd())).unwrap();
        totals.add(Money::new(500, usd())).unwrap();
        assert_eq!(
            serde_json::to_value(&totals).unwrap(),
            json!([
                {"currency": "IQD", "minor": 1_000, "digits": 0, "text": "1000"},
                {"currency": "USD", "minor": 3_000, "digits": 2, "text": "30.00"},
            ])
        );
        assert_eq!(totals.in_currency(usd()).minor, 3_000);
        assert_eq!(
            totals.in_currency(Currency::of("EUR").unwrap()).minor,
            0,
            "nothing was owed in euros, which is a real zero"
        );
    }

    #[test]
    fn a_conversion_keeps_the_rate_in_the_direction_it_was_quoted() {
        // 1,320 dinars to the dollar.
        let rate = Conversion {
            from: usd(),
            to: iqd(),
            rate: 1_320,
            at: 1_700_000_000_000,
        };
        // $25.00 is 33,000 dinars...
        let forward = rate.apply(Money::new(2_500, usd())).unwrap();
        assert_eq!(forward.amount, Money::new(33_000, iqd()));
        assert!(forward.exact);
        // ...and 33,000 dinars settles $25.00, by dividing rather than by a
        // second rate that would have rounded to nothing.
        let back = rate.apply(Money::new(33_000, iqd())).unwrap();
        assert_eq!(back.amount, Money::new(2_500, usd()));
        assert!(back.exact);
        // A rounding is reported rather than hidden.
        let short = rate.apply(Money::new(32_999, iqd())).unwrap();
        assert_eq!(short.amount, Money::new(2_500, usd()));
        assert!(!short.exact, "a minor unit was rounded away");
        // An amount in a third currency is an error.
        assert!(
            rate.apply(Money::new(100, Currency::of("EUR").unwrap()))
                .is_err()
        );
    }

    #[test]
    fn a_rate_is_a_whole_number_entered_by_a_person() {
        let at = 1_700_000_000_000;
        let input: ConversionInput =
            serde_json::from_value(json!({"from": "USD", "to": "IQD", "rate": 1_320})).unwrap();
        assert_eq!(input.resolve(at).unwrap().at, at, "the payment's moment");
        for bad in [json!(1320.5), json!("1320"), json!(0), json!(-5)] {
            let input: ConversionInput =
                serde_json::from_value(json!({"from": "USD", "to": "IQD", "rate": bad})).unwrap();
            assert!(input.resolve(at).is_err(), "{bad} is not a rate");
        }
        let same: ConversionInput =
            serde_json::from_value(json!({"from": "USD", "to": "USD", "rate": 1})).unwrap();
        assert!(same.resolve(at).is_err(), "a rate needs two currencies");
    }

    #[test]
    fn converting_a_total_needs_a_rate_for_every_currency_in_it() {
        let mut totals = Totals::new();
        totals.add(Money::new(2_500, usd())).unwrap();
        totals.add(Money::new(33_000, iqd())).unwrap();
        let err = convert_totals(&totals, usd(), &[], 1).unwrap_err();
        assert!(err.message.contains("no rate between"), "{}", err.message);
        let rate = Conversion {
            from: usd(),
            to: iqd(),
            rate: 1_320,
            at: 1,
        };
        let converted = convert_totals(&totals, usd(), &[rate], 2).unwrap();
        assert_eq!(converted.amount, Money::new(5_000, usd()));
        assert_eq!(converted.basis, "conversion");
        assert_eq!(converted.rates, vec![rate]);
        assert_eq!(converted.of, totals, "the ledger travels with the figure");
        assert_eq!(converted.at, 2);
    }

    #[test]
    fn scaling_rounds_half_away_from_zero_and_says_when_it_rounded() {
        let price = Money::new(10_000, usd());
        // A yearly price as a month: exact.
        let monthly = price.scale_by(1, 4).unwrap();
        assert_eq!(monthly.amount.minor, 2_500);
        assert!(monthly.exact);
        // Half a period of an odd amount: rounded, and reported.
        let half = Money::new(2_501, usd()).scale_by(1, 2).unwrap();
        assert_eq!(half.amount.minor, 1_251);
        assert!(!half.exact);
        let negative = Money::new(-2_501, usd()).scale_by(1, 2).unwrap();
        assert_eq!(negative.amount.minor, -1_251);
        assert!(price.scale_by(1, 0).is_err());
    }
}
