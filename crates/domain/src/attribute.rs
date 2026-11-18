use std::{cmp::Ordering, collections::BTreeMap, str::FromStr};

/// JSON-shaped data. Absence is represented by `Value::Absent`, not `Null`.
/// Objects compare without key order; arrays retain order and duplicates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<AttributeValue>),
    Object(BTreeMap<String, AttributeValue>),
}

/// Exact decimal number, normalized for equality without binary floating point.
/// The coefficient has arbitrary length; the normalized decimal exponent is i64.
/// This is a domain representation, not a JSON codec or a wire format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Number {
    negative: bool,
    coefficient: String,
    exponent: i64,
}

impl Number {
    /// The sign, significant digits and exponent of this normalized decimal.
    pub fn decimal_parts(&self) -> (bool, &str, i64) {
        (self.negative, &self.coefficient, self.exponent)
    }

    fn compare_magnitude(&self, other: &Self) -> Ordering {
        let self_nonzero = self.coefficient != "0";
        let other_nonzero = other.coefficient != "0";
        if !self_nonzero || !other_nonzero {
            return self_nonzero.cmp(&other_nonzero);
        }
        // Compare the positions of the leading digits without expanding either exponent.
        let self_scale = i128::from(self.exponent) + self.coefficient.len() as i128;
        let other_scale = i128::from(other.exponent) + other.coefficient.len() as i128;
        self_scale.cmp(&other_scale).then_with(|| {
            // With aligned leading digits, a shorter prefix compares as zero-padded:
            // the longer normalized coefficient has a nonzero final digit.
            self.coefficient.cmp(&other.coefficient)
        })
    }
}

impl Ord for Number {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => other.compare_magnitude(self),
            (false, false) => self.compare_magnitude(other),
        }
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberError {
    InvalidSyntax,
    ExponentOutOfRange,
}

impl std::fmt::Display for NumberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for NumberError {}

impl FromStr for Number {
    type Err = NumberError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        use NumberError::*;
        let (negative, input) = match input.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, input),
        };
        let (mantissa, exponent) = match input.split_once(['e', 'E']) {
            Some((mantissa, exp)) => {
                let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
                if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(InvalidSyntax);
                }
                (
                    mantissa,
                    exp.parse::<i64>().map_err(|_| ExponentOutOfRange)?,
                )
            }
            None => (input, 0),
        };
        let (integer, fraction) = match mantissa.split_once('.') {
            Some((integer, fraction)) if !fraction.is_empty() => (integer, fraction),
            Some(_) => return Err(InvalidSyntax),
            None => (mantissa, ""),
        };
        if integer.is_empty()
            || (integer.len() > 1 && integer.starts_with('0'))
            || !integer
                .bytes()
                .chain(fraction.bytes())
                .all(|c| c.is_ascii_digit())
        {
            return Err(InvalidSyntax);
        }
        let digits = format!("{integer}{fraction}");
        let significant = digits.trim_start_matches('0');
        if significant.is_empty() {
            return Ok(Self {
                negative: false,
                coefficient: "0".into(),
                exponent: 0,
            });
        }
        let coefficient = significant.trim_end_matches('0');
        // Use a wider intermediate so cancellation at the i64 boundaries works.
        let exponent = i128::from(exponent) - fraction.len() as i128
            + (significant.len() - coefficient.len()) as i128;
        Ok(Self {
            negative,
            coefficient: coefficient.into(),
            exponent: exponent.try_into().map_err(|_| ExponentOutOfRange)?,
        })
    }
}

impl std::fmt::Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&self.coefficient)?;
        if self.exponent != 0 {
            write!(f, "e{}", self.exponent)?;
        }
        Ok(())
    }
}

macro_rules! number_from_integer {
    ($($ty:ty),+) => { $(
        impl From<$ty> for Number {
            fn from(value: $ty) -> Self {
                value.to_string().parse().expect("integer is a valid decimal number")
            }
        }
    )+ };
}
number_from_integer!(i64, u64);
