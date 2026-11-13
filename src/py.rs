//! Python number semantics: the output's numbers are defined by them.
//!
//! The conversion mixes ints and floats as Python does (`0 * -5.0` is `-0.0`, `-0 * 1.0` is `0.0`), sums with the
//! builtin `sum()` (CPython 3.12+: Neumaier-compensated, starting from the int 0) and prints floats with `repr()`
//! (the shortest round-trip digits). `Num` carries the int / float distinction through the arithmetic so signed zeros
//! and roundings come out the same.

use std::cmp::Ordering;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// A Python number: an int or a float (the converter never needs anything else).
#[derive(Clone, Copy, Debug)]
pub enum Num {
    I(i64),
    F(f64),
}

pub use Num::{F, I};

impl Num {
    pub fn f(self) -> f64 {
        match self {
            I(i) => i as f64,
            F(f) => f,
        }
    }
    /// Python truthiness.
    pub fn truthy(self) -> bool {
        match self {
            I(i) => i != 0,
            F(f) => f != 0.0,
        }
    }
    /// `x or default`.
    pub fn or(self, d: Num) -> Num {
        if self.truthy() { self } else { d }
    }
    pub fn abs(self) -> Num {
        match self {
            I(i) => I(i.abs()),
            F(f) => F(f.abs()),
        }
    }
    /// Equality as Python's `==` / hashing (`-0.0 == 0.0 == 0`): the key used to dedupe.
    pub fn key(self) -> u64 {
        let f = self.f();
        if f == 0.0 { 0 } else { f.to_bits() }
    }
    pub fn lt(self, o: Num) -> bool {
        match (self, o) {
            (I(a), I(b)) => a < b,
            _ => self.f() < o.f(),
        }
    }
    pub fn gt(self, o: Num) -> bool {
        o.lt(self)
    }
}

impl From<i64> for Num {
    fn from(v: i64) -> Num {
        I(v)
    }
}
impl From<i32> for Num {
    fn from(v: i32) -> Num {
        I(v as i64)
    }
}
impl From<f64> for Num {
    fn from(v: f64) -> Num {
        F(v)
    }
}

impl Add for Num {
    type Output = Num;
    fn add(self, o: Num) -> Num {
        match (self, o) {
            (I(a), I(b)) => I(a + b),
            _ => F(self.f() + o.f()),
        }
    }
}
impl Sub for Num {
    type Output = Num;
    fn sub(self, o: Num) -> Num {
        match (self, o) {
            (I(a), I(b)) => I(a - b),
            _ => F(self.f() - o.f()),
        }
    }
}
impl Mul for Num {
    type Output = Num;
    fn mul(self, o: Num) -> Num {
        match (self, o) {
            (I(a), I(b)) => I(a * b),
            _ => F(self.f() * o.f()),
        }
    }
}
/// Python's true division `/` (always a float).
impl Div for Num {
    type Output = Num;
    fn div(self, o: Num) -> Num {
        F(self.f() / o.f())
    }
}
impl Neg for Num {
    type Output = Num;
    fn neg(self) -> Num {
        match self {
            I(a) => I(-a),
            F(f) => F(-f),
        }
    }
}
impl PartialEq for Num {
    fn eq(&self, o: &Num) -> bool {
        match (*self, *o) {
            (I(a), I(b)) => a == b,
            _ => self.f() == o.f(),
        }
    }
}
impl PartialOrd for Num {
    fn partial_cmp(&self, o: &Num) -> Option<Ordering> {
        match (*self, *o) {
            (I(a), I(b)) => a.partial_cmp(&b),
            _ => self.f().partial_cmp(&o.f()),
        }
    }
}

/// The builtin `sum(items)` of CPython 3.12+: ints summed exactly until the first float, then the floats (and any
/// later ints) with Neumaier's compensated summation.
pub fn sum<I2: IntoIterator<Item = Num>>(items: I2) -> Num {
    let mut it = items.into_iter();
    let mut acc: i64 = 0;
    let (mut hi, mut lo);
    loop {
        match it.next() {
            None => return I(acc),
            Some(I(v)) => acc += v,
            Some(F(x)) => {
                hi = acc as f64 + x;
                lo = 0.0;
                break;
            }
        }
    }
    for item in it {
        let x = item.f();
        let t = hi + x;
        if hi.abs() >= x.abs() {
            lo += (hi - t) + x;
        } else {
            lo += (x - t) + hi;
        }
        hi = t;
    }
    if lo != 0.0 && lo.is_finite() { F(hi + lo) } else { F(hi) }
}

/// `min(a, b)` / `max(a, b)`: the first argument unless the second is strictly smaller / larger.
pub fn min2(a: Num, b: Num) -> Num {
    if b.lt(a) { b } else { a }
}
pub fn max2(a: Num, b: Num) -> Num {
    if b.gt(a) { b } else { a }
}

pub fn sqrt(x: Num) -> Num {
    F(x.f().sqrt())
}
pub fn sin(x: Num) -> Num {
    F(libm::sin(x.f()))
}
pub fn cos(x: Num) -> Num {
    F(libm::cos(x.f()))
}
pub fn acos(x: Num) -> Num {
    F(libm::acos(x.f()))
}

/// `repr(float)`: the shortest digits that read back as the same double, laid out as CPython does (exponent form
/// below 1e-4 and from 1e16 on).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-Infinity".into() } else { "Infinity".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let s = format!("{:e}", x.abs());
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1;
    let n = digits.len() as i32;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!("e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs()));
    } else if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..-decpt {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decpt >= n {
        out.push_str(&digits);
        for _ in 0..decpt - n {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `round(x, nd)` for a float: the correctly rounded decimal (half-even on the exact binary value), read back.
pub fn round(x: f64, nd: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let r: f64 = format!("{:.*}", nd, x).parse().unwrap();
    // CPython keeps the sign of a zero result (round(-0.00001, 4) is -0.0)
    if r == 0.0 { 0.0f64.copysign(x) } else { r }
}

/// `round(x)` with no digits: the nearest integer, ties to even.
pub fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_like_python() {
        assert_eq!(float_repr(1e-05), "1e-05");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1e15), "1000000000000000.0");
        assert_eq!(float_repr(123456789012345678.0), "1.2345678901234568e+17");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(1.0 / 300.0), "0.0033333333333333335");
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(float_repr(-1.5e-7), "-1.5e-07");
    }

    #[test]
    fn sum_like_python() {
        assert_eq!(sum([F(1e16), F(1.0), F(1.0)]).f(), 1.0000000000000002e16);
        assert_eq!(sum([F(1e16), I(1), I(1)]).f(), 1.0000000000000002e16);
        assert!(sum([F(-0.0)]).f().is_sign_positive());
        assert!(matches!(sum([I(0), I(2)]), I(2)));
        assert!(matches!(I(0) * F(-5.0), F(f) if f.is_sign_negative()));
        assert!(matches!(-I(0) * F(1.0), F(f) if f.is_sign_positive()));
    }

    #[test]
    fn round_like_python() {
        assert_eq!(round(0.25, 1), 0.2);
        assert_eq!(round(1.0 / 3.0, 4), 0.3333);
        assert_eq!(round_int(2.5), 2);
    }
}
