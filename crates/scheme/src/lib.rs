//! A small Scheme interpreter in safe Rust: the R5RS core plus the TinyScheme-style extras that
//! Script-Fu scripts rely on (`define` with rest arguments, named `let`, `do`, `case`, `assq`
//! and friends, vectors and `cons-array`, `strbreakup`, `random`).
//!
//! It is sandboxed by construction: no file, network or process access of its own. All outside
//! effects go through a [`Host`], which resolves procedure and constant names the script uses but
//! the interpreter does not define (Script-Fu's `gimp-*` procedures). A step budget and a
//! recursion limit end runaway scripts with an [`Error`]; nothing here panics on any input.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod builtins;
mod eval;
mod reader;
mod value;

use std::rc::Rc;

pub use reader::Reader;
pub use value::{Builtin, Env, Value, eqv, equal};

/// Longest list the interpreter will walk or print.
pub const MAX_LIST: usize = 1_000_000;
/// Longest text `write` / `display` produce for one value.
pub const MAX_PRINT: usize = 1 << 20;

/// A Scheme error, with the message the script's author sees.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

impl Error {
    pub fn msg(m: impl Into<String>) -> Error {
        Error(m.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Resolves what the script uses but the interpreter doesn't define.
pub trait Host {
    /// A constant (`RGB`, `NORMAL-MODE`, …); `None` if `name` is not one.
    fn constant(&mut self, _name: &str) -> Option<Value> {
        None
    }
    /// A procedure call with evaluated arguments; `None` if `name` is not a host procedure.
    fn call(&mut self, interp: &mut Interp, name: &str, args: &[Value]) -> Option<Result<Value>>;
}

/// An interpreter instance: a global environment plus its limits and output buffer.
pub struct Interp {
    pub global: Env,
    pub(crate) steps: u64,
    pub max_steps: u64,
    pub(crate) depth: usize,
    pub max_depth: usize,
    pub(crate) out: String,
    pub(crate) rng: u64,
    pub(crate) host: Option<Box<dyn Host>>,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        let mut i = Interp {
            global: value::new_env(None),
            steps: 0,
            max_steps: 50_000_000,
            depth: 0,
            max_depth: 600,
            out: String::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            host: None,
        };
        builtins::install(&mut i);
        i
    }

    /// Installs the host that resolves unknown procedures and constants.
    pub fn set_host(&mut self, host: Box<dyn Host>) {
        self.host = Some(host);
    }

    /// Defines a global variable.
    pub fn define(&mut self, name: &str, v: Value) {
        self.global.vars.borrow_mut().insert(Rc::from(name), v);
    }

    /// Reads a global variable.
    pub fn get(&self, name: &str) -> Option<Value> {
        value::lookup(&self.global, name)
    }

    /// Text printed by `display` / `write` / `newline` since the last call.
    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.out)
    }

    /// Seeds `random`.
    pub fn seed_random(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    /// Reads and evaluates every form of `src`; the value of the last one. The step budget
    /// restarts for each call.
    pub fn eval_str(&mut self, src: &str) -> Result<Value> {
        self.steps = 0;
        self.depth = 0;
        let mut reader = Reader::new(src);
        let mut last = Value::Unspecified;
        let env = self.global.clone();
        while let Some(form) = reader.read()? {
            last = self.eval(&form, &env)?;
        }
        Ok(last)
    }

    /// Calls a global procedure by name.
    pub fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        let f = self.get(name).ok_or_else(|| Error::msg(format!("unbound variable: {name}")))?;
        self.apply_value(&f, args)
    }
}

#[cfg(test)]
mod tests;
