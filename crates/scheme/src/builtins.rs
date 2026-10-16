//! Built-in procedures.

use std::rc::Rc;

use crate::value::{Builtin, Value, equal, eqv, format_float};
use crate::{Error, Interp, MAX_LIST, Result};

type F = fn(&mut Interp, &[Value]) -> Result<Value>;

fn reg(i: &mut Interp, name: &'static str, min: usize, max: Option<usize>, f: F) {
    i.define(name, Value::Builtin(Builtin { name, min, max, f }));
}

fn at(a: &[Value], i: usize) -> Result<&Value> {
    a.get(i).ok_or_else(|| Error::msg("missing argument"))
}
fn int(v: &Value) -> Result<i64> {
    v.as_int().ok_or_else(|| Error::msg(format!("expected an integer, got {}", v.write())))
}
fn num(v: &Value) -> Result<f64> {
    v.as_f64().ok_or_else(|| Error::msg(format!("expected a number, got {}", v.write())))
}
fn text(v: &Value) -> Result<String> {
    v.as_string().ok_or_else(|| Error::msg(format!("expected a string, got {}", v.write())))
}
fn index(v: &Value) -> Result<usize> {
    usize::try_from(int(v)?).map_err(|_| Error::msg("index out of range"))
}
fn boolean(b: bool) -> Result<Value> {
    Ok(Value::Bool(b))
}

// ------------------------------------------------------------------ numbers

fn arith(a: &Value, b: &Value, int_op: fn(i64, i64) -> Option<i64>, float_op: fn(f64, f64) -> f64) -> Result<Value> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(int_op(*x, *y).map_or_else(|| Value::Float(float_op(*x as f64, *y as f64)), Value::Int)),
        _ => Ok(Value::Float(float_op(num(a)?, num(b)?))),
    }
}

fn add(_: &mut Interp, a: &[Value]) -> Result<Value> {
    a.iter().try_fold(Value::Int(0), |acc, v| arith(&acc, v, i64::checked_add, |x, y| x + y))
}
fn mul(_: &mut Interp, a: &[Value]) -> Result<Value> {
    a.iter().try_fold(Value::Int(1), |acc, v| arith(&acc, v, i64::checked_mul, |x, y| x * y))
}
fn sub(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let first = at(a, 0)?;
    if a.len() == 1 {
        return arith(&Value::Int(0), first, i64::checked_sub, |x, y| x - y);
    }
    a.iter().skip(1).try_fold(first.clone(), |acc, v| arith(&acc, v, i64::checked_sub, |x, y| x - y))
}
fn div(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let divide = |x: &Value, y: &Value| -> Result<Value> {
        if let (Value::Int(p), Value::Int(q)) = (x, y)
            && *q != 0
            && p % q == 0
        {
            return Ok(Value::Int(p / q));
        }
        let d = num(y)?;
        if d == 0.0 {
            return Err(Error::msg("division by zero"));
        }
        Ok(Value::Float(num(x)? / d))
    };
    let first = at(a, 0)?;
    if a.len() == 1 {
        return divide(&Value::Int(1), first);
    }
    a.iter().skip(1).try_fold(first.clone(), |acc, v| divide(&acc, v))
}

fn compare(a: &[Value], ok: fn(std::cmp::Ordering) -> bool) -> Result<Value> {
    for w in a.windows(2) {
        let (x, y) = (at(w, 0)?, at(w, 1)?);
        let ord = match (x, y) {
            (Value::Int(p), Value::Int(q)) => p.cmp(q),
            _ => num(x)?.partial_cmp(&num(y)?).ok_or_else(|| Error::msg("cannot compare NaN"))?,
        };
        if !ok(ord) {
            return boolean(false);
        }
    }
    boolean(true)
}

fn int_div(a: &[Value], f: fn(i64, i64) -> Option<i64>, name: &str) -> Result<Value> {
    let (x, y) = (int(at(a, 0)?)?, int(at(a, 1)?)?);
    if y == 0 {
        return Err(Error::msg(format!("{name}: division by zero")));
    }
    f(x, y).map(Value::Int).ok_or_else(|| Error::msg(format!("{name}: overflow")))
}

/// R5RS `modulo`: the result takes the divisor's sign.
fn floor_mod(x: i64, y: i64) -> Option<i64> {
    let r = x.checked_rem(y)?;
    Some(if r != 0 && (r < 0) != (y < 0) { r + y } else { r })
}

fn float_fn(a: &[Value], f: fn(f64) -> f64) -> Result<Value> {
    Ok(Value::Float(f(num(at(a, 0)?)?)))
}

/// Rounds like R5RS `round` (halves to even), keeping integers integer.
fn round_with(a: &[Value], f: fn(f64) -> f64) -> Result<Value> {
    match at(a, 0)? {
        Value::Int(i) => Ok(Value::Int(*i)),
        v => Ok(Value::Float(f(num(v)?))),
    }
}

fn number_to_string(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let radix = a.get(1).map(int).transpose()?.unwrap_or(10);
    match at(a, 0)? {
        Value::Int(i) if radix == 10 => Ok(Value::string(i.to_string())),
        Value::Int(i) if (2..=16).contains(&radix) => {
            let mut n = i.unsigned_abs();
            let mut digits = Vec::new();
            while n > 0 {
                digits.push(std::char::from_digit((n % radix as u64) as u32, radix as u32).unwrap_or('0'));
                n /= radix as u64;
            }
            if digits.is_empty() {
                digits.push('0');
            }
            if *i < 0 {
                digits.push('-');
            }
            Ok(Value::string(digits.iter().rev().collect::<String>()))
        }
        Value::Float(f) => Ok(Value::string(format_float(*f))),
        v => Err(Error::msg(format!("number->string: bad argument {}", v.write()))),
    }
}

fn string_to_number(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let s = text(at(a, 0)?)?;
    let radix = a.get(1).map(int).transpose()?.unwrap_or(10);
    if radix != 10 {
        return Ok(u32::try_from(radix).ok().and_then(|r| i64::from_str_radix(s.trim(), r).ok()).map_or(Value::Bool(false), Value::Int));
    }
    Ok(crate::reader::parse_number(s.trim()).unwrap_or(Value::Bool(false)))
}

fn random(i: &mut Interp, a: &[Value]) -> Result<Value> {
    // xorshift64*: deterministic for a given seed.
    i.rng ^= i.rng >> 12;
    i.rng ^= i.rng << 25;
    i.rng ^= i.rng >> 27;
    let bits = i.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
    match at(a, 0)? {
        Value::Int(n) if *n > 0 => Ok(Value::Int((bits % *n as u64) as i64)),
        Value::Float(f) if *f > 0.0 => Ok(Value::Float((bits >> 11) as f64 / (1u64 << 53) as f64 * f)),
        _ => Err(Error::msg("random: expected a positive number")),
    }
}

// ------------------------------------------------------------------ lists

fn cxr(v: &Value, path: &str) -> Result<Value> {
    path.chars().rev().try_fold(v.clone(), |cur, c| {
        let next = if c == 'a' { cur.car() } else { cur.cdr() };
        next.ok_or_else(|| Error::msg(format!("c{path}r: not a pair: {}", cur.write())))
    })
}

fn append(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let Some((last, init)) = a.split_last() else { return Ok(Value::Nil) };
    let mut items = Vec::new();
    for l in init {
        items.extend(l.to_vec()?);
    }
    Ok(items.into_iter().rev().fold(last.clone(), |t, v| Value::cons(v, t)))
}

fn member_by(a: &[Value], same: fn(&Value, &Value) -> bool) -> Result<Value> {
    let key = at(a, 0)?;
    let mut cur = at(a, 1)?.clone();
    let mut n = 0;
    while let Value::Pair(p) = &cur {
        if same(key, &p.car.borrow()) {
            return Ok(cur.clone());
        }
        n += 1;
        if n > MAX_LIST {
            return Err(Error::msg("list too long (or circular)"));
        }
        let next = p.cdr.borrow().clone();
        cur = next;
    }
    boolean(false)
}

fn assoc_by(a: &[Value], same: fn(&Value, &Value) -> bool) -> Result<Value> {
    let key = at(a, 0)?;
    for entry in at(a, 1)?.to_vec()? {
        if entry.car().is_some_and(|k| same(key, &k)) {
            return Ok(entry);
        }
    }
    boolean(false)
}

fn map(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let f = at(a, 0)?.clone();
    let lists: Vec<Vec<Value>> = a.iter().skip(1).map(Value::to_vec).collect::<Result<_>>()?;
    let n = lists.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let args: Vec<Value> = lists.iter().filter_map(|l| l.get(k).cloned()).collect();
        out.push(i.apply_value(&f, args)?);
    }
    Ok(Value::list(out))
}

fn for_each(i: &mut Interp, a: &[Value]) -> Result<Value> {
    map(i, a)?;
    Ok(Value::Unspecified)
}

fn apply(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let f = at(a, 0)?.clone();
    let mut args: Vec<Value> = a.get(1..a.len().saturating_sub(1)).unwrap_or_default().to_vec();
    if a.len() > 1 {
        args.extend(at(a, a.len() - 1)?.to_vec()?);
    }
    i.apply_value(&f, args)
}

fn filter(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let f = at(a, 0)?.clone();
    let mut out = Vec::new();
    for v in at(a, 1)?.to_vec()? {
        if i.apply_value(&f, vec![v.clone()])?.truthy() {
            out.push(v);
        }
    }
    Ok(Value::list(out))
}

fn fold_left(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let f = at(a, 0)?.clone();
    let mut acc = at(a, 1)?.clone();
    for v in at(a, 2)?.to_vec()? {
        acc = i.apply_value(&f, vec![acc, v])?;
    }
    Ok(acc)
}

fn fold_right(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let f = at(a, 0)?.clone();
    let mut acc = at(a, 1)?.clone();
    for v in at(a, 2)?.to_vec()?.into_iter().rev() {
        acc = i.apply_value(&f, vec![v, acc])?;
    }
    Ok(acc)
}

fn list_tail(a: &[Value]) -> Result<Value> {
    let mut cur = at(a, 0)?.clone();
    for _ in 0..index(at(a, 1)?)? {
        cur = cur.cdr().ok_or_else(|| Error::msg("list-tail: list too short"))?;
    }
    Ok(cur)
}

fn iota(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let n = index(at(a, 0)?)?;
    if n > MAX_LIST {
        return Err(Error::msg("iota: too long"));
    }
    let start = a.get(1).map(int).transpose()?.unwrap_or(0);
    let step = a.get(2).map(int).transpose()?.unwrap_or(1);
    Ok(Value::list((0..n as i64).map(|k| Value::Int(start.saturating_add(k.saturating_mul(step)))).collect::<Vec<_>>()))
}

// ------------------------------------------------------------------ vectors

fn vec_of(v: &Value) -> Result<Rc<std::cell::RefCell<Vec<Value>>>> {
    match v {
        Value::Vector(items) => Ok(items.clone()),
        other => Err(Error::msg(format!("expected a vector, got {}", other.write()))),
    }
}

fn make_vector(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let n = index(at(a, 0)?)?;
    if n > MAX_LIST {
        return Err(Error::msg("make-vector: too long"));
    }
    Ok(Value::vector(vec![a.get(1).cloned().unwrap_or(Value::Int(0)); n]))
}

fn vector_ref(_: &mut Interp, a: &[Value]) -> Result<Value> {
    vec_of(at(a, 0)?)?.borrow().get(index(at(a, 1)?)?).cloned().ok_or_else(|| Error::msg("vector index out of range"))
}

fn vector_set(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let v = vec_of(at(a, 0)?)?;
    let mut v = v.borrow_mut();
    let slot = v.get_mut(index(at(a, 1)?)?).ok_or_else(|| Error::msg("vector index out of range"))?;
    *slot = at(a, 2)?.clone();
    Ok(Value::Unspecified)
}

// ------------------------------------------------------------------ strings and characters

fn string_cmp(a: &[Value], fold: bool, ok: fn(std::cmp::Ordering) -> bool) -> Result<Value> {
    for w in a.windows(2) {
        let (mut x, mut y) = (text(at(w, 0)?)?, text(at(w, 1)?)?);
        if fold {
            x = x.to_lowercase();
            y = y.to_lowercase();
        }
        if !ok(x.cmp(&y)) {
            return boolean(false);
        }
    }
    boolean(true)
}

fn substring(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let s: Vec<char> = text(at(a, 0)?)?.chars().collect();
    let start = index(at(a, 1)?)?;
    let end = a.get(2).map(index).transpose()?.unwrap_or(s.len());
    s.get(start..end).map(|x| Value::string(x.iter().collect::<String>())).ok_or_else(|| Error::msg("substring: range out of bounds"))
}

fn string_append(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let mut out = String::new();
    for v in a {
        out.push_str(&text(v)?);
        if out.len() > crate::MAX_PRINT * 8 {
            return Err(Error::msg("string too long"));
        }
    }
    Ok(Value::string(out))
}

fn strbreakup(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let (s, sep) = (text(at(a, 0)?)?, text(at(a, 1)?)?);
    if sep.is_empty() {
        return Ok(Value::list([Value::string(s)]));
    }
    Ok(Value::list(s.split(sep.as_str()).map(Value::string).collect::<Vec<_>>()))
}

fn unbreakupstr(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let sep = text(at(a, 1)?)?;
    let parts: Vec<String> = at(a, 0)?.to_vec()?.iter().map(text).collect::<Result<_>>()?;
    Ok(Value::string(parts.join(&sep)))
}

fn chr(v: &Value) -> Result<char> {
    match v {
        Value::Char(c) => Ok(*c),
        other => Err(Error::msg(format!("expected a character, got {}", other.write()))),
    }
}

// ------------------------------------------------------------------ output and control

fn display(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let s = at(a, 0)?.display();
    i.out.push_str(&s);
    Ok(Value::Unspecified)
}

fn write(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let s = at(a, 0)?.write();
    i.out.push_str(&s);
    Ok(Value::Unspecified)
}

fn error(_: &mut Interp, a: &[Value]) -> Result<Value> {
    let msg = a.iter().map(Value::display).collect::<Vec<_>>().join(" ");
    Err(Error::msg(msg))
}

fn eval_builtin(i: &mut Interp, a: &[Value]) -> Result<Value> {
    let env = i.global.clone();
    i.eval(at(a, 0)?, &env)
}

pub(crate) fn install(i: &mut Interp) {
    // numbers
    reg(i, "+", 0, None, add);
    reg(i, "*", 0, None, mul);
    reg(i, "-", 1, None, sub);
    reg(i, "/", 1, None, div);
    reg(i, "=", 1, None, |_, a| compare(a, |o| o.is_eq()));
    reg(i, "<", 1, None, |_, a| compare(a, |o| o.is_lt()));
    reg(i, ">", 1, None, |_, a| compare(a, |o| o.is_gt()));
    reg(i, "<=", 1, None, |_, a| compare(a, |o| o.is_le()));
    reg(i, ">=", 1, None, |_, a| compare(a, |o| o.is_ge()));
    reg(i, "quotient", 2, Some(2), |_, a| int_div(a, i64::checked_div, "quotient"));
    reg(i, "remainder", 2, Some(2), |_, a| int_div(a, i64::checked_rem, "remainder"));
    reg(i, "modulo", 2, Some(2), |_, a| int_div(a, floor_mod, "modulo"));
    reg(i, "abs", 1, Some(1), |_, a| match at(a, 0)? {
        Value::Int(x) => Ok(x.checked_abs().map_or(Value::Float((*x as f64).abs()), Value::Int)),
        v => Ok(Value::Float(num(v)?.abs())),
    });
    reg(i, "min", 1, None, |_, a| extreme(a, true));
    reg(i, "max", 1, None, |_, a| extreme(a, false));
    reg(i, "floor", 1, Some(1), |_, a| round_with(a, f64::floor));
    reg(i, "ceiling", 1, Some(1), |_, a| round_with(a, f64::ceil));
    reg(i, "truncate", 1, Some(1), |_, a| round_with(a, f64::trunc));
    reg(i, "round", 1, Some(1), |_, a| round_with(a, f64::round_ties_even));
    reg(i, "sqrt", 1, Some(1), |_, a| {
        let x = num(at(a, 0)?)?;
        if x >= 0.0 && x.fract() == 0.0 && x < 4.0e15 {
            let r = x.sqrt();
            if r.fract() == 0.0 {
                return Ok(Value::Int(r as i64));
            }
        }
        Ok(Value::Float(x.sqrt()))
    });
    reg(i, "expt", 2, Some(2), |_, a| match (at(a, 0)?, at(a, 1)?) {
        (Value::Int(b), Value::Int(e)) if *e >= 0 => u32::try_from(*e).ok().and_then(|e| b.checked_pow(e)).map_or_else(|| Ok(Value::Float((*b as f64).powf(*e as f64))), |v| Ok(Value::Int(v))),
        (b, e) => Ok(Value::Float(num(b)?.powf(num(e)?))),
    });
    reg(i, "exp", 1, Some(1), |_, a| float_fn(a, f64::exp));
    reg(i, "log", 1, Some(1), |_, a| float_fn(a, f64::ln));
    reg(i, "sin", 1, Some(1), |_, a| float_fn(a, f64::sin));
    reg(i, "cos", 1, Some(1), |_, a| float_fn(a, f64::cos));
    reg(i, "tan", 1, Some(1), |_, a| float_fn(a, f64::tan));
    reg(i, "asin", 1, Some(1), |_, a| float_fn(a, f64::asin));
    reg(i, "acos", 1, Some(1), |_, a| float_fn(a, f64::acos));
    reg(i, "atan", 1, Some(2), |_, a| match a.get(1) {
        Some(x) => Ok(Value::Float(num(at(a, 0)?)?.atan2(num(x)?))),
        None => float_fn(a, f64::atan),
    });
    reg(i, "number->string", 1, Some(2), number_to_string);
    reg(i, "string->number", 1, Some(2), string_to_number);
    reg(i, "exact->inexact", 1, Some(1), |_, a| Ok(Value::Float(num(at(a, 0)?)?)));
    reg(i, "inexact", 1, Some(1), |_, a| Ok(Value::Float(num(at(a, 0)?)?)));
    reg(i, "inexact->exact", 1, Some(1), |_, a| {
        let f = num(at(a, 0)?)?;
        if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 { Ok(Value::Int(f as i64)) } else { Err(Error::msg("inexact->exact: not an integer value")) }
    });
    reg(i, "exact", 1, Some(1), |i, a| i.call("inexact->exact", a.to_vec()));
    reg(i, "zero?", 1, Some(1), |_, a| boolean(num(at(a, 0)?)? == 0.0));
    reg(i, "positive?", 1, Some(1), |_, a| boolean(num(at(a, 0)?)? > 0.0));
    reg(i, "negative?", 1, Some(1), |_, a| boolean(num(at(a, 0)?)? < 0.0));
    reg(i, "odd?", 1, Some(1), |_, a| boolean(int(at(a, 0)?)? % 2 != 0));
    reg(i, "even?", 1, Some(1), |_, a| boolean(int(at(a, 0)?)? % 2 == 0));
    reg(i, "number?", 1, Some(1), |_, a| boolean(at(a, 0)?.as_f64().is_some()));
    reg(i, "real?", 1, Some(1), |_, a| boolean(at(a, 0)?.as_f64().is_some()));
    reg(i, "integer?", 1, Some(1), |_, a| boolean(at(a, 0)?.as_int().is_some()));
    reg(i, "random", 1, Some(1), random);
    // equivalence and types
    reg(i, "not", 1, Some(1), |_, a| boolean(!at(a, 0)?.truthy()));
    reg(i, "eq?", 2, Some(2), |_, a| boolean(eqv(at(a, 0)?, at(a, 1)?)));
    reg(i, "eqv?", 2, Some(2), |_, a| boolean(eqv(at(a, 0)?, at(a, 1)?)));
    reg(i, "equal?", 2, Some(2), |_, a| boolean(equal(at(a, 0)?, at(a, 1)?)));
    reg(i, "null?", 1, Some(1), |_, a| boolean(at(a, 0)?.is_nil()));
    reg(i, "pair?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Pair(_))));
    reg(i, "list?", 1, Some(1), |_, a| boolean(at(a, 0)?.to_vec().is_ok()));
    reg(i, "symbol?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Sym(_))));
    reg(i, "string?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Str(_))));
    reg(i, "boolean?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Bool(_))));
    reg(i, "char?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Char(_))));
    reg(i, "vector?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Vector(_))));
    reg(i, "procedure?", 1, Some(1), |_, a| boolean(matches!(at(a, 0)?, Value::Closure(_) | Value::Builtin(_))));
    // lists
    reg(i, "cons", 2, Some(2), |_, a| Ok(Value::cons(at(a, 0)?.clone(), at(a, 1)?.clone())));
    reg(i, "car", 1, Some(1), |_, a| cxr(at(a, 0)?, "a"));
    reg(i, "cdr", 1, Some(1), |_, a| cxr(at(a, 0)?, "d"));
    reg(i, "caar", 1, Some(1), |_, a| cxr(at(a, 0)?, "aa"));
    reg(i, "cadr", 1, Some(1), |_, a| cxr(at(a, 0)?, "ad"));
    reg(i, "cdar", 1, Some(1), |_, a| cxr(at(a, 0)?, "da"));
    reg(i, "cddr", 1, Some(1), |_, a| cxr(at(a, 0)?, "dd"));
    reg(i, "caddr", 1, Some(1), |_, a| cxr(at(a, 0)?, "add"));
    reg(i, "cdddr", 1, Some(1), |_, a| cxr(at(a, 0)?, "ddd"));
    reg(i, "cadddr", 1, Some(1), |_, a| cxr(at(a, 0)?, "addd"));
    reg(i, "set-car!", 2, Some(2), |_, a| match at(a, 0)? {
        Value::Pair(p) => {
            *p.car.borrow_mut() = at(a, 1)?.clone();
            Ok(Value::Unspecified)
        }
        v => Err(Error::msg(format!("set-car!: not a pair: {}", v.write()))),
    });
    reg(i, "set-cdr!", 2, Some(2), |_, a| match at(a, 0)? {
        Value::Pair(p) => {
            *p.cdr.borrow_mut() = at(a, 1)?.clone();
            Ok(Value::Unspecified)
        }
        v => Err(Error::msg(format!("set-cdr!: not a pair: {}", v.write()))),
    });
    reg(i, "list", 0, None, |_, a| Ok(Value::list(a.to_vec())));
    reg(i, "length", 1, Some(1), |_, a| Ok(Value::Int(at(a, 0)?.to_vec()?.len() as i64)));
    reg(i, "append", 0, None, append);
    reg(i, "reverse", 1, Some(1), |_, a| Ok(Value::list(at(a, 0)?.to_vec()?.into_iter().rev().collect::<Vec<_>>())));
    reg(i, "list-ref", 2, Some(2), |_, a| {
        let items = at(a, 0)?.to_vec()?;
        items.get(index(at(a, 1)?)?).cloned().ok_or_else(|| Error::msg("list-ref: index out of range"))
    });
    reg(i, "list-tail", 2, Some(2), |_, a| list_tail(a));
    reg(i, "list-copy", 1, Some(1), |_, a| Ok(Value::list(at(a, 0)?.to_vec()?)));
    reg(i, "last-pair", 1, Some(1), |_, a| {
        let mut cur = at(a, 0)?.clone();
        while let Some(next) = cur.cdr().filter(|n| matches!(n, Value::Pair(_))) {
            cur = next;
        }
        Ok(cur)
    });
    reg(i, "memq", 2, Some(2), |_, a| member_by(a, eqv));
    reg(i, "memv", 2, Some(2), |_, a| member_by(a, eqv));
    reg(i, "member", 2, Some(2), |_, a| member_by(a, equal));
    reg(i, "assq", 2, Some(2), |_, a| assoc_by(a, eqv));
    reg(i, "assv", 2, Some(2), |_, a| assoc_by(a, eqv));
    reg(i, "assoc", 2, Some(2), |_, a| assoc_by(a, equal));
    reg(i, "map", 2, None, map);
    reg(i, "for-each", 2, None, for_each);
    reg(i, "apply", 1, None, apply);
    reg(i, "filter", 2, Some(2), filter);
    reg(i, "fold-left", 3, Some(3), fold_left);
    reg(i, "fold-right", 3, Some(3), fold_right);
    reg(i, "iota", 1, Some(3), iota);
    // vectors (Script-Fu arrays are vectors)
    reg(i, "vector", 0, None, |_, a| Ok(Value::vector(a.to_vec())));
    reg(i, "make-vector", 1, Some(2), make_vector);
    reg(i, "cons-array", 1, Some(2), |i, a| make_vector(i, a.get(..1).unwrap_or_default()));
    reg(i, "vector-ref", 2, Some(2), vector_ref);
    reg(i, "aref", 2, Some(2), vector_ref);
    reg(i, "vector-set!", 3, Some(3), vector_set);
    reg(i, "aset", 3, Some(3), vector_set);
    reg(i, "vector-length", 1, Some(1), |_, a| Ok(Value::Int(vec_of(at(a, 0)?)?.borrow().len() as i64)));
    reg(i, "vector->list", 1, Some(1), |_, a| Ok(Value::list(vec_of(at(a, 0)?)?.borrow().clone())));
    reg(i, "list->vector", 1, Some(1), |_, a| Ok(Value::vector(at(a, 0)?.to_vec()?)));
    reg(i, "vector-fill!", 2, Some(2), |_, a| {
        let v = vec_of(at(a, 0)?)?;
        let fill = at(a, 1)?.clone();
        v.borrow_mut().iter_mut().for_each(|x| *x = fill.clone());
        Ok(Value::Unspecified)
    });
    // strings and characters
    reg(i, "string-length", 1, Some(1), |_, a| Ok(Value::Int(text(at(a, 0)?)?.chars().count() as i64)));
    reg(i, "string-ref", 2, Some(2), |_, a| {
        text(at(a, 0)?)?.chars().nth(index(at(a, 1)?)?).map(Value::Char).ok_or_else(|| Error::msg("string-ref: index out of range"))
    });
    reg(i, "substring", 2, Some(3), substring);
    reg(i, "string-append", 0, None, string_append);
    reg(i, "string=?", 1, None, |_, a| string_cmp(a, false, |o| o.is_eq()));
    reg(i, "string<?", 1, None, |_, a| string_cmp(a, false, |o| o.is_lt()));
    reg(i, "string>?", 1, None, |_, a| string_cmp(a, false, |o| o.is_gt()));
    reg(i, "string-ci=?", 1, None, |_, a| string_cmp(a, true, |o| o.is_eq()));
    reg(i, "string-upcase", 1, Some(1), |_, a| Ok(Value::string(text(at(a, 0)?)?.to_uppercase())));
    reg(i, "string-downcase", 1, Some(1), |_, a| Ok(Value::string(text(at(a, 0)?)?.to_lowercase())));
    reg(i, "string-copy", 1, Some(1), |_, a| Ok(Value::string(text(at(a, 0)?)?)));
    reg(i, "make-string", 1, Some(2), |_, a| {
        let n = index(at(a, 0)?)?;
        if n > crate::MAX_PRINT {
            return Err(Error::msg("make-string: too long"));
        }
        let c = a.get(1).map(chr).transpose()?.unwrap_or(' ');
        Ok(Value::string(std::iter::repeat_n(c, n).collect::<String>()))
    });
    reg(i, "string->symbol", 1, Some(1), |_, a| Ok(Value::sym(&text(at(a, 0)?)?)));
    reg(i, "symbol->string", 1, Some(1), |_, a| {
        at(a, 0)?.as_sym().map(Value::string).ok_or_else(|| Error::msg("symbol->string: not a symbol"))
    });
    reg(i, "string->list", 1, Some(1), |_, a| Ok(Value::list(text(at(a, 0)?)?.chars().map(Value::Char).collect::<Vec<_>>())));
    reg(i, "list->string", 1, Some(1), |_, a| {
        Ok(Value::string(at(a, 0)?.to_vec()?.iter().map(chr).collect::<Result<String>>()?))
    });
    reg(i, "strbreakup", 2, Some(2), strbreakup);
    reg(i, "unbreakupstr", 2, Some(2), unbreakupstr);
    reg(i, "char->integer", 1, Some(1), |_, a| Ok(Value::Int(i64::from(u32::from(chr(at(a, 0)?)?)))));
    reg(i, "integer->char", 1, Some(1), |_, a| {
        u32::try_from(int(at(a, 0)?)?).ok().and_then(char::from_u32).map(Value::Char).ok_or_else(|| Error::msg("integer->char: bad code point"))
    });
    reg(i, "char-upcase", 1, Some(1), |_, a| Ok(Value::Char(chr(at(a, 0)?)?.to_uppercase().next().unwrap_or(' '))));
    reg(i, "char-downcase", 1, Some(1), |_, a| Ok(Value::Char(chr(at(a, 0)?)?.to_lowercase().next().unwrap_or(' '))));
    reg(i, "char-alphabetic?", 1, Some(1), |_, a| boolean(chr(at(a, 0)?)?.is_alphabetic()));
    reg(i, "char-numeric?", 1, Some(1), |_, a| boolean(chr(at(a, 0)?)?.is_numeric()));
    reg(i, "char-whitespace?", 1, Some(1), |_, a| boolean(chr(at(a, 0)?)?.is_whitespace()));
    reg(i, "char=?", 2, Some(2), |_, a| boolean(chr(at(a, 0)?)? == chr(at(a, 1)?)?));
    // output and control
    reg(i, "display", 1, Some(2), display);
    reg(i, "write", 1, Some(2), write);
    reg(i, "newline", 0, Some(1), |i, _| {
        i.out.push('\n');
        Ok(Value::Unspecified)
    });
    reg(i, "error", 0, None, error);
    reg(i, "eval", 1, Some(2), eval_builtin);
}

fn extreme(a: &[Value], min: bool) -> Result<Value> {
    let mut best = at(a, 0)?.clone();
    let mut inexact = matches!(best, Value::Float(_));
    for v in a.iter().skip(1) {
        inexact |= matches!(v, Value::Float(_));
        let better = if min { num(v)? < num(&best)? } else { num(v)? > num(&best)? };
        if better {
            best = v.clone();
        }
    }
    Ok(if inexact { Value::Float(num(&best)?) } else { best })
}
