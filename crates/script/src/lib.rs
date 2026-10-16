//! Script-Fu for PhotoCraft: Scheme scripts (the TinyScheme dialect GIMP's Script-Fu uses) that
//! drive the engine through GIMP-compatible procedure names.
//!
//! Scripts keep their `gimp-*` names so existing ones run, but each procedure is a thin mapping
//! onto a PhotoCraft *command* ([`pdb`]); nothing reaches past the engine's command layer, so
//! scripts get the same selection, lock, history and colour-model handling as the menus.
//!
//! The interpreter ([`photocraft_scheme`]) has no file, network or process access. A script runs
//! against the session handed to [`ScriptFu::run`]; it is one history step per command, like any
//! other automation.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod pdb;
mod registry;

use std::cell::RefCell;
use std::rc::Rc;

use photocraft_engine::Session;
use photocraft_scheme::{Interp, Value};

pub use pdb::{constant, constant_names};
pub use registry::{ParamKind, ParamSpec, Registered};

/// What a script run produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Text the script printed (`display`, `gimp-message`).
    pub output: String,
    /// The last expression's value as `write` would print it (empty when it has none).
    pub value: String,
}

/// One Script-Fu interpreter: its definitions persist between runs, as in GIMP's console.
pub struct ScriptFu {
    interp: Interp,
    shared: Rc<RefCell<pdb::Shared>>,
}

impl Default for ScriptFu {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptFu {
    pub fn new() -> Self {
        let shared = Rc::new(RefCell::new(pdb::Shared::default()));
        let mut interp = Interp::new();
        interp.set_host(Box::new(pdb::Pdb(shared.clone())));
        ScriptFu { interp, shared }
    }

    /// Runs `source` against `session`. Scripts that fail leave the session as the commands they
    /// already ran left it (each is its own history step, so Undo works).
    pub fn run(&mut self, session: &mut Session, source: &str) -> Result<Outcome, String> {
        self.with_session(session, |this| this.interp.eval_str(source).map(|v| v.write()))
    }

    /// Scripts registered with `script-fu-register` so far.
    pub fn registered(&self) -> Vec<Registered> {
        self.shared.borrow().registered.clone()
    }

    /// Runs the registered script `name` with `args`, the way the Run dialog does.
    pub fn run_registered(&mut self, session: &mut Session, name: &str, args: Vec<Value>) -> Result<Outcome, String> {
        self.with_session(session, |this| this.interp.call(name, args).map(|v| v.write()))
    }

    fn with_session(&mut self, session: &mut Session, f: impl FnOnce(&mut Self) -> photocraft_scheme::Result<String>) -> Result<Outcome, String> {
        self.begin(session);
        // A panic inside a command must not lose the user's documents: they are handed back below
        // whatever happens, and the panic becomes an error.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)))
            .unwrap_or_else(|_| Err(photocraft_scheme::Error::msg("Script-Fu failed with an internal error; your open documents are unchanged")));
        let mut output = self.interp.take_output();
        output.push_str(&self.end(session));
        match result {
            Ok(value) => Ok(Outcome { output, value }),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Hands `session` to the procedure database for a run. Pair with [`ScriptFu::end`]; other
    /// languages (Python-Fu) drive the same procedures through [`ScriptFu::procedures`] between
    /// the two.
    pub fn begin(&mut self, session: &mut Session) {
        let taken = std::mem::replace(session, Session::new());
        let mut sh = self.shared.borrow_mut();
        sh.session = Some(taken);
        sh.output.clear();
    }

    /// Takes the session back; returns what `gimp-message` printed.
    pub fn end(&mut self, session: &mut Session) -> String {
        let mut sh = self.shared.borrow_mut();
        if let Some(back) = sh.session.take() {
            *session = back;
        }
        std::mem::take(&mut sh.output)
    }

    /// A handle for calling procedures while a session is attached.
    pub fn procedures(&self) -> Procedures {
        Procedures(self.shared.clone())
    }
}

/// Calls the procedure database (`gimp-image-new`, `plug-in-gauss`, …) of a [`ScriptFu`].
#[derive(Clone)]
pub struct Procedures(Rc<RefCell<pdb::Shared>>);

impl Procedures {
    /// Calls `name` with `args`; the result is the list of the procedure's return values.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value, String> {
        match self.0.borrow_mut().call(name, args) {
            Some(r) => r.map_err(|e| e.to_string()),
            None => Err(format!("no procedure named {name}")),
        }
    }

    /// Records a script registered from another language.
    pub fn register(&self, script: Registered) {
        let mut sh = self.0.borrow_mut();
        sh.registered.retain(|r| r.name != script.name);
        sh.registered.push(script);
    }

    /// Adds text to the run's output (what `gimp.message` prints).
    pub fn print(&self, text: &str) {
        self.0.borrow_mut().output.push_str(text);
    }
}

#[cfg(test)]
mod tests;
