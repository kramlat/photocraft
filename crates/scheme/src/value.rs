//! Scheme values, environments and the printer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::{Error, Interp, Result};

/// A Scheme value. Cloning is cheap (reference counted).
#[derive(Clone)]
pub enum Value {
    Nil,
    /// The value of expressions with no useful result (`set!`, `for-each`, …); prints as nothing.
    Unspecified,
    Bool(bool),
    Int(i64),
    Float(f64),
    Char(char),
    Str(Rc<RefCell<String>>),
    Sym(Rc<str>),
    Pair(Rc<Pair>),
    Vector(Rc<RefCell<Vec<Value>>>),
    Closure(Rc<Closure>),
    Builtin(Builtin),
}

pub struct Pair {
    pub car: RefCell<Value>,
    pub cdr: RefCell<Value>,
}

impl Drop for Pair {
    /// Drops a long list iteratively, so a 100 000-element list can't overflow the stack.
    fn drop(&mut self) {
        let mut next = std::mem::replace(self.cdr.get_mut(), Value::Nil);
        while let Value::Pair(rc) = next {
            match Rc::try_unwrap(rc) {
                Ok(mut p) => next = std::mem::replace(p.cdr.get_mut(), Value::Nil),
                Err(_) => break,
            }
        }
    }
}

pub struct Closure {
    pub params: Vec<Rc<str>>,
    pub rest: Option<Rc<str>>,
    pub body: Vec<Value>,
    pub env: Env,
    pub name: RefCell<Option<Rc<str>>>,
}

/// A native procedure.
#[derive(Clone)]
pub struct Builtin {
    pub name: &'static str,
    pub min: usize,
    /// `None` = any number of further arguments.
    pub max: Option<usize>,
    pub f: fn(&mut Interp, &[Value]) -> Result<Value>,
}

/// A lexical frame.
pub struct Frame {
    pub vars: RefCell<HashMap<Rc<str>, Value>>,
    pub parent: Option<Env>,
}

pub type Env = Rc<Frame>;

pub fn new_env(parent: Option<Env>) -> Env {
    Rc::new(Frame { vars: RefCell::new(HashMap::new()), parent })
}

/// Looks `name` up through the chain of frames.
pub fn lookup(env: &Env, name: &str) -> Option<Value> {
    let mut cur = Some(env);
    while let Some(f) = cur {
        if let Some(v) = f.vars.borrow().get(name) {
            return Some(v.clone());
        }
        cur = f.parent.as_ref();
    }
    None
}

/// Assigns to the nearest existing binding; `false` when there is none.
pub fn assign(env: &Env, name: &str, value: Value) -> bool {
    let mut cur = Some(env);
    while let Some(f) = cur {
        if let Some(slot) = f.vars.borrow_mut().get_mut(name) {
            *slot = value;
            return true;
        }
        cur = f.parent.as_ref();
    }
    false
}

impl Value {
    pub fn cons(a: Value, d: Value) -> Value {
        Value::Pair(Rc::new(Pair { car: RefCell::new(a), cdr: RefCell::new(d) }))
    }
    pub fn sym(s: &str) -> Value {
        Value::Sym(Rc::from(s))
    }
    pub fn string(s: impl Into<String>) -> Value {
        Value::Str(Rc::new(RefCell::new(s.into())))
    }
    pub fn vector(v: Vec<Value>) -> Value {
        Value::Vector(Rc::new(RefCell::new(v)))
    }
    /// A proper list of `items`.
    pub fn list(items: impl IntoIterator<Item = Value, IntoIter: DoubleEndedIterator>) -> Value {
        items.into_iter().rev().fold(Value::Nil, |tail, v| Value::cons(v, tail))
    }
    /// Everything but `#f` is true.
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Bool(false))
    }
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }
    pub fn car(&self) -> Option<Value> {
        match self {
            Value::Pair(p) => Some(p.car.borrow().clone()),
            _ => None,
        }
    }
    pub fn cdr(&self) -> Option<Value> {
        match self {
            Value::Pair(p) => Some(p.cdr.borrow().clone()),
            _ => None,
        }
    }
    pub fn as_sym(&self) -> Option<&str> {
        match self {
            Value::Sym(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 && f.is_finite() && f.abs() < 9.0e15 => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_string(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(s.borrow().clone()),
            _ => None,
        }
    }
    /// The elements of a proper list; an error for an improper or circular one.
    pub fn to_vec(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cur = self.clone();
        loop {
            match cur {
                Value::Nil => return Ok(out),
                Value::Pair(p) => {
                    if out.len() >= crate::MAX_LIST {
                        return Err(Error::msg("list too long (or circular)"));
                    }
                    out.push(p.car.borrow().clone());
                    let next = p.cdr.borrow().clone();
                    cur = next;
                }
                other => return Err(Error::msg(format!("improper list ending in {}", other.write()))),
            }
        }
    }

    /// `display` form: strings and characters without quotes.
    pub fn display(&self) -> String {
        let mut out = String::new();
        print(self, false, &mut out, 0);
        out
    }
    /// `write` form: strings quoted and escaped.
    pub fn write(&self) -> String {
        let mut out = String::new();
        print(self, true, &mut out, 0);
        out
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.write())
    }
}

/// `eqv?` for atoms, identity for heap objects.
pub fn eqv(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) | (Value::Unspecified, Value::Unspecified) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Sym(x), Value::Sym(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y),
        (Value::Pair(x), Value::Pair(y)) => Rc::ptr_eq(x, y),
        (Value::Vector(x), Value::Vector(y)) => Rc::ptr_eq(x, y),
        (Value::Closure(x), Value::Closure(y)) => Rc::ptr_eq(x, y),
        (Value::Builtin(x), Value::Builtin(y)) => x.name == y.name,
        _ => false,
    }
}

/// `equal?`: structural, with a depth bound.
pub fn equal(a: &Value, b: &Value) -> bool {
    fn go(a: &Value, b: &Value, depth: usize) -> bool {
        if depth > 10_000 {
            return false;
        }
        match (a, b) {
            (Value::Str(x), Value::Str(y)) => *x.borrow() == *y.borrow(),
            (Value::Pair(x), Value::Pair(y)) => {
                Rc::ptr_eq(x, y) || (go(&x.car.borrow(), &y.car.borrow(), depth + 1) && go(&x.cdr.borrow(), &y.cdr.borrow(), depth + 1))
            }
            (Value::Vector(x), Value::Vector(y)) => {
                let (x, y) = (x.borrow(), y.borrow());
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| go(p, q, depth + 1))
            }
            _ => eqv(a, b),
        }
    }
    go(a, b, 0)
}

pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        "+nan.0".into()
    } else if f.is_infinite() {
        if f > 0.0 { "+inf.0".into() } else { "-inf.0".into() }
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn print(v: &Value, quote: bool, out: &mut String, depth: usize) {
    if depth > 200 || out.len() > crate::MAX_PRINT {
        out.push_str("...");
        return;
    }
    match v {
        Value::Nil => out.push_str("()"),
        Value::Unspecified => {}
        Value::Bool(true) => out.push_str("#t"),
        Value::Bool(false) => out.push_str("#f"),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => out.push_str(&format_float(*f)),
        Value::Char(c) if quote => match c {
            ' ' => out.push_str("#\\space"),
            '\n' => out.push_str("#\\newline"),
            '\t' => out.push_str("#\\tab"),
            c => {
                out.push_str("#\\");
                out.push(*c);
            }
        },
        Value::Char(c) => out.push(*c),
        Value::Str(s) if quote => {
            out.push('"');
            for c in s.borrow().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Str(s) => out.push_str(&s.borrow()),
        Value::Sym(s) => out.push_str(s),
        Value::Pair(_) => {
            // (quote x) prints as 'x
            if let (Some(Value::Sym(head)), Some(Value::Pair(rest))) = (v.car(), v.cdr())
                && &*head == "quote"
                && rest.cdr.borrow().is_nil()
            {
                out.push('\'');
                print(&rest.car.borrow(), quote, out, depth + 1);
                return;
            }
            out.push('(');
            let mut cur = v.clone();
            let mut first = true;
            let mut count = 0usize;
            loop {
                match cur {
                    Value::Pair(p) => {
                        if !first {
                            out.push(' ');
                        }
                        first = false;
                        count += 1;
                        if count > crate::MAX_LIST {
                            out.push_str("...");
                            break;
                        }
                        print(&p.car.borrow(), quote, out, depth + 1);
                        let next = p.cdr.borrow().clone();
                        cur = next;
                    }
                    Value::Nil => break,
                    other => {
                        out.push_str(" . ");
                        print(&other, quote, out, depth + 1);
                        break;
                    }
                }
            }
            out.push(')');
        }
        Value::Vector(items) => {
            out.push_str("#(");
            for (i, x) in items.borrow().iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                print(x, quote, out, depth + 1);
            }
            out.push(')');
        }
        Value::Closure(c) => {
            out.push_str("#<procedure");
            if let Some(n) = c.name.borrow().as_ref() {
                out.push(' ');
                out.push_str(n);
            }
            out.push('>');
        }
        Value::Builtin(b) => {
            out.push_str("#<builtin ");
            out.push_str(b.name);
            out.push('>');
        }
    }
}
