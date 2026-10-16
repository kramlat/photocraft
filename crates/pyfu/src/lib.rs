//! Python-Fu for PhotoCraft: runs Python scripts written for GIMP's Python-Fu (`from gimpfu import
//! *`, `pdb.gimp_image_new(...)`, `register(...)`) against the PhotoCraft engine.
//!
//! The interpreter is RustPython without its standard library: a script has no file, network,
//! process or module-import access, only the language itself plus the names this crate provides.
//! `pdb.<name_with_underscores>` calls the same procedure database as Script-Fu
//! ([`photocraft_script`]), each procedure being a thin mapping onto a PhotoCraft command.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::RefCell;
use std::rc::Rc;

use photocraft_engine::Session;
use photocraft_scheme::Value;
use photocraft_script::{Outcome, ParamKind, ParamSpec, Procedures, Registered, ScriptFu};
use rustpython_vm::builtins::{PyFloat, PyInt, PyList, PyStr, PyTuple};
use rustpython_vm::function::FuncArgs;
use rustpython_vm::scope::Scope;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyResult, VirtualMachine};

/// Names of the Python-Fu parameter kinds (`PF_*`), in the order their numeric codes run.
const PF: &[(&str, ParamKind)] = &[
    ("PF_INT", ParamKind::Adjustment),
    ("PF_FLOAT", ParamKind::Adjustment),
    ("PF_STRING", ParamKind::String),
    ("PF_COLOR", ParamKind::Color),
    ("PF_TOGGLE", ParamKind::Toggle),
    ("PF_SLIDER", ParamKind::Adjustment),
    ("PF_SPINNER", ParamKind::Adjustment),
    ("PF_IMAGE", ParamKind::Image),
    ("PF_DRAWABLE", ParamKind::Drawable),
    ("PF_FILE", ParamKind::String),
    ("PF_FONT", ParamKind::Other),
    ("PF_OPTION", ParamKind::Other),
    ("PF_RADIO", ParamKind::Other),
    ("PF_BRUSH", ParamKind::Other),
    ("PF_PATTERN", ParamKind::Other),
    ("PF_GRADIENT", ParamKind::Other),
];

/// Aliases for the kinds above.
const PF_ALIASES: &[(&str, &str)] = &[
    ("PF_BOOL", "PF_TOGGLE"),
    ("PF_LAYER", "PF_DRAWABLE"),
    ("PF_CHANNEL", "PF_DRAWABLE"),
    ("PF_VALUE", "PF_FLOAT"),
    ("PF_DIRNAME", "PF_FILE"),
    ("PF_TEXT", "PF_STRING"),
];

const PRELUDE: &str = r#"
class _PDB(object):
    def __getattr__(self, name):
        proc = name.replace('_', '-')
        def call(*args):
            return _pdb_call(proc, list(args))
        return call

pdb = _PDB()

class _Gimp(object):
    pdb = pdb
    def message(self, text):
        _pdb_call('gimp-message', [str(text)])
    def displays_flush(self):
        pass
    def context_push(self):
        pass
    def context_pop(self):
        pass
    def progress_init(self, *args):
        pass
    def progress_update(self, *args):
        pass

gimp = _Gimp()
_registered = {}

def register(proc_name, blurb, help, author, copyright, date, label, imagetypes,
             params, results, function, menu=None, domain=None, on_query=None, on_run=None):
    _registered[proc_name] = function
    _pf_register(proc_name, label, menu or '', blurb,
                 [(int(p[0]), str(p[1]), str(p[2]), repr(p[3]) if len(p) > 3 else '') for p in params])

def main():
    pass
"#;

/// A Python-Fu interpreter: definitions persist between runs, as in an interactive console.
pub struct PythonFu {
    fu: ScriptFu,
    interp: Interpreter,
    scope: Scope,
    output: Rc<RefCell<String>>,
}

impl Default for PythonFu {
    fn default() -> Self {
        Self::new()
    }
}

fn type_error(vm: &VirtualMachine, msg: impl Into<String>) -> rustpython_vm::builtins::PyBaseExceptionRef {
    vm.new_type_error(msg.into())
}

/// Python object → Scheme value (the interchange the procedure database speaks).
fn to_value(vm: &VirtualMachine, obj: &PyObjectRef) -> PyResult<Value> {
    if vm.is_none(obj) {
        return Ok(Value::Nil);
    }
    if obj.class().is(vm.ctx.types.bool_type) {
        return Ok(Value::Bool(obj.clone().try_to_bool(vm)?));
    }
    if let Some(i) = obj.downcast_ref::<PyInt>() {
        return i.try_to_primitive::<i64>(vm).map(Value::Int);
    }
    if let Some(f) = obj.downcast_ref::<PyFloat>() {
        return Ok(Value::Float(f.to_f64()));
    }
    if let Some(s) = obj.downcast_ref::<PyStr>() {
        return Ok(Value::string(s.as_str()));
    }
    if let Some(l) = obj.downcast_ref::<PyList>() {
        let items: Vec<PyObjectRef> = l.borrow_vec().to_vec();
        return items.iter().map(|x| to_value(vm, x)).collect::<PyResult<Vec<_>>>().map(Value::list);
    }
    if let Some(t) = obj.downcast_ref::<PyTuple>() {
        return t.as_slice().iter().map(|x| to_value(vm, x)).collect::<PyResult<Vec<_>>>().map(Value::list);
    }
    Err(type_error(vm, format!("cannot pass a {} to a procedure", obj.class().name())))
}

/// Scheme value → Python object.
fn to_py(vm: &VirtualMachine, v: &Value) -> PyObjectRef {
    match v {
        Value::Nil | Value::Unspecified => vm.ctx.none(),
        Value::Bool(b) => vm.ctx.new_bool(*b).into(),
        Value::Int(i) => vm.ctx.new_int(*i).into(),
        Value::Float(f) => vm.ctx.new_float(*f).into(),
        Value::Char(c) => vm.ctx.new_str(c.to_string()).into(),
        Value::Str(s) => vm.ctx.new_str(s.borrow().as_str()).into(),
        Value::Sym(s) => vm.ctx.new_str(&**s).into(),
        Value::Pair(_) => match v.to_vec() {
            Ok(items) => vm.ctx.new_list(items.iter().map(|x| to_py(vm, x)).collect()).into(),
            Err(_) => vm.ctx.none(),
        },
        Value::Vector(items) => vm.ctx.new_list(items.borrow().iter().map(|x| to_py(vm, x)).collect()).into(),
        Value::Closure(_) | Value::Builtin(_) => vm.ctx.none(),
    }
}

/// A procedure's result list as Python-Fu returns it: one value alone, several as a tuple.
fn result_to_py(vm: &VirtualMachine, v: &Value) -> PyObjectRef {
    match v.to_vec() {
        Ok(items) if items.len() == 1 => to_py(vm, &items[0]),
        Ok(items) => vm.ctx.new_tuple(items.iter().map(|x| to_py(vm, x)).collect()).into(),
        Err(_) => to_py(vm, v),
    }
}

fn kind_of(code: i64) -> ParamKind {
    usize::try_from(code).ok().and_then(|i| PF.get(i)).map_or(ParamKind::Other, |(_, k)| *k)
}

/// GIMP's own modules: their names are already defined, so importing them is a no-op.
const GIMP_MODULES: &[&str] = &["gimpfu", "gimp", "gimpenums", "gimpshelf", "gimpplugin"];

/// Drops the lines importing GIMP's modules and refuses every other import: the sandbox has no
/// module system, and host modules (`_io`, `posix`, …) must stay out of reach.
fn strip_gimp_imports(src: &str) -> Result<String, String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        let module = if let Some(rest) = t.strip_prefix("import ") {
            Some(rest)
        } else {
            t.strip_prefix("from ").filter(|r| r.contains(" import "))
        };
        let Some(module) = module else {
            out.push(line.to_string());
            continue;
        };
        let names: Vec<&str> = if t.starts_with("from ") {
            vec![module.split_whitespace().next().unwrap_or_default()]
        } else {
            module.split(',').map(|m| m.split_whitespace().next().unwrap_or_default()).collect()
        };
        if let Some(bad) = names.iter().find(|n| !GIMP_MODULES.contains(&n.split('.').next().unwrap_or_default())) {
            return Err(format!("ImportError: importing '{bad}' is not available in Python-Fu"));
        }
        out.push(format!("{}pass", &line[..line.len() - t.len()]));
    }
    Ok(out.join("\n"))
}

impl PythonFu {
    pub fn new() -> Self {
        let fu = ScriptFu::new();
        let output = Rc::new(RefCell::new(String::new()));
        let interp = Interpreter::without_stdlib(Default::default());
        let procs = fu.procedures();
        let scope = interp.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            install(vm, &scope, &procs, &output);
            scope
        });
        PythonFu { fu, interp, scope, output }
    }

    /// Runs `source` against `session`. An expression's value comes back as its `repr`.
    pub fn run(&mut self, session: &mut Session, source: &str) -> Result<Outcome, String> {
        let source = strip_gimp_imports(source)?;
        self.attached(session, |this| this.exec(&source))
    }

    /// Scripts registered with `register(...)` so far (Script-Fu's too).
    pub fn registered(&self) -> Vec<Registered> {
        self.fu.registered()
    }

    /// Runs the registered script `name` with `args` (numbers, strings, booleans, lists).
    pub fn run_registered(&mut self, session: &mut Session, name: &str, args: Vec<Value>) -> Result<Outcome, String> {
        let name = name.to_string();
        self.attached(session, move |this| {
            let scope = this.scope.clone();
            this.interp.enter(|vm| {
                let table = scope.globals.get_item("_registered", vm).map_err(|e| describe(vm, &e))?;
                let f = table.get_item(name.as_str(), vm).map_err(|_| format!("no Python script named {name}"))?;
                let py_args: Vec<PyObjectRef> = args.iter().map(|a| to_py(vm, a)).collect();
                let r = f.call(py_args, vm).map_err(|e| describe(vm, &e))?;
                value_repr(vm, &r)
            })
        })
    }

    fn attached(&mut self, session: &mut Session, f: impl FnOnce(&mut Self) -> Result<String, String>) -> Result<Outcome, String> {
        self.fu.begin(session);
        self.output.borrow_mut().clear();
        // Whatever happens inside, the documents go back to the caller.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)))
            .unwrap_or_else(|_| Err("Python-Fu failed with an internal error; your open documents are unchanged".into()));
        let mut output = std::mem::take(&mut *self.output.borrow_mut());
        output.push_str(&self.fu.end(session));
        result.map(|value| Outcome { output, value })
    }

    fn exec(&mut self, source: &str) -> Result<String, String> {
        let scope = self.scope.clone();
        self.interp.enter(|vm| {
            use rustpython_vm::compiler::Mode;
            // An expression answers with its value, as a console does; statements run.
            if let Ok(code) = vm.compile(source, Mode::Eval, "<python-fu>".to_owned()) {
                let r = vm.run_code_obj(code, scope).map_err(|e| describe(vm, &e))?;
                return value_repr(vm, &r);
            }
            // A script ending in an unindented expression line answers with that line's value.
            if let Some((head, last)) = source.trim_end().rsplit_once('\n')
                && !last.starts_with([' ', '\t'])
                && let Ok(tail) = vm.compile(last, Mode::Eval, "<python-fu>".to_owned())
                && let Ok(body) = vm.compile(head, Mode::Exec, "<python-fu>".to_owned())
            {
                vm.run_code_obj(body, scope.clone()).map_err(|e| describe(vm, &e))?;
                let r = vm.run_code_obj(tail, scope).map_err(|e| describe(vm, &e))?;
                return value_repr(vm, &r);
            }
            let code = vm.compile(source, Mode::Exec, "<python-fu>".to_owned()).map_err(|e| format!("SyntaxError: {e}"))?;
            vm.run_code_obj(code, scope).map_err(|e| describe(vm, &e))?;
            Ok(String::new())
        })
    }
}

fn value_repr(vm: &VirtualMachine, r: &PyObjectRef) -> Result<String, String> {
    if vm.is_none(r) {
        return Ok(String::new());
    }
    r.repr(vm).map(|s| s.as_str().to_string()).map_err(|e| describe(vm, &e))
}

/// A Python exception as the text a user reads (`ValueError: …`).
fn describe(vm: &VirtualMachine, e: &rustpython_vm::builtins::PyBaseExceptionRef) -> String {
    let mut text = String::new();
    if vm.write_exception(&mut text, e).is_err() || text.trim().is_empty() {
        return format!("{}", e.class().name());
    }
    text.trim().to_string()
}

fn install(vm: &VirtualMachine, scope: &Scope, procs: &Procedures, output: &Rc<RefCell<String>>) {
    let set = |name: &str, v: PyObjectRef| {
        let _ = scope.globals.set_item(name, v, vm);
    };
    // Procedure calls.
    let p = procs.clone();
    set(
        "_pdb_call",
        vm.new_function("_pdb_call", move |args: FuncArgs, vm: &VirtualMachine| -> PyResult {
            let name = args.args.first().and_then(|n| n.downcast_ref::<PyStr>().map(|s| s.as_str().to_string())).ok_or_else(|| type_error(vm, "procedure name expected"))?;
            let list = args.args.get(1).ok_or_else(|| type_error(vm, "argument list expected"))?;
            let values = match to_value(vm, list)?.to_vec() {
                Ok(v) => v,
                Err(_) => return Err(type_error(vm, "argument list expected")),
            };
            match p.call(&name, &values) {
                Ok(r) => Ok(result_to_py(vm, &r)),
                Err(e) => Err(vm.new_runtime_error(e)),
            }
        })
        .into(),
    );
    // register(): remember the script for the Plugins menu.
    let p = procs.clone();
    set(
        "_pf_register",
        vm.new_function("_pf_register", move |args: FuncArgs, vm: &VirtualMachine| -> PyResult {
            let s = |i: usize| args.args.get(i).and_then(|a| a.downcast_ref::<PyStr>().map(|x| x.as_str().to_string())).unwrap_or_default();
            let mut params = Vec::new();
            if let Some(list) = args.args.get(4)
                && let Ok(items) = to_value(vm, list).and_then(|v| v.to_vec().map_err(|e| type_error(vm, e.to_string())))
            {
                for it in items {
                    let f = it.to_vec().unwrap_or_default();
                    params.push(ParamSpec {
                        kind: kind_of(f.first().and_then(Value::as_int).unwrap_or(-1)),
                        label: f.get(2).and_then(Value::as_string).filter(|d| !d.is_empty()).or_else(|| f.get(1).and_then(Value::as_string)).unwrap_or_default(),
                        default: f.get(3).and_then(Value::as_string).unwrap_or_default(),
                    });
                }
            }
            let menu = s(2);
            let label = s(1);
            p.register(Registered {
                name: s(0),
                label: label.rsplit('/').next().unwrap_or_default().to_string(),
                menu: if menu.is_empty() { label.rsplit_once('/').map(|(m, _)| m.to_string()) } else { Some(menu.rsplit_once('/').map_or(menu.clone(), |(m, _)| m.to_string())) },
                description: s(3),
                params,
            });
            Ok(vm.ctx.none())
        })
        .into(),
    );
    // print(): collected instead of written to a console that isn't there.
    let out = output.clone();
    set(
        "print",
        vm.new_function("print", move |args: FuncArgs, vm: &VirtualMachine| -> PyResult {
            let mut line = String::new();
            for (i, a) in args.args.iter().enumerate() {
                if i > 0 {
                    line.push(' ');
                }
                line.push_str(a.str(vm)?.as_str());
            }
            let mut o = out.borrow_mut();
            o.push_str(&line);
            o.push('\n');
            Ok(vm.ctx.none())
        })
        .into(),
    );
    // Constants: RGB, NORMAL_MODE, FILL_FOREGROUND-style names with underscores.
    for name in photocraft_script::constant_names() {
        if let Some(v) = photocraft_script::constant(name) {
            set(&name.replace('-', "_"), vm.ctx.new_int(v).into());
        }
    }
    for (i, (name, _)) in PF.iter().enumerate() {
        set(name, vm.ctx.new_int(i as i64).into());
    }
    for (alias, of) in PF_ALIASES {
        if let Some(i) = PF.iter().position(|(n, _)| n == of) {
            set(alias, vm.ctx.new_int(i as i64).into());
        }
    }
    // GIMP's Python names for the Script-Fu fills and a few common aliases.
    for (name, v) in [("FILL_FOREGROUND", 0), ("FILL_BACKGROUND", 1), ("FILL_WHITE", 2), ("FILL_TRANSPARENT", 3), ("FILL_PATTERN", 4), ("TRUE", 1), ("FALSE", 0)] {
        set(name, vm.ctx.new_int(v).into());
    }
    // Close the doors to the host: files, imports, dynamic code and the console. (The scripts are
    // code the user chose to run, as in GIMP; this keeps accidents and casual reach-through out,
    // it is not a boundary against a determined attacker.)
    let builtins = vm.builtins.dict();
    for name in ["open", "__import__", "exec", "eval", "compile", "input", "breakpoint", "help", "exit", "quit", "copyright", "credits", "license"] {
        let _ = builtins.del_item(name, vm);
    }
    if let Ok(code) = vm.compile(PRELUDE, rustpython_vm::compiler::Mode::Exec, "<python-fu prelude>".to_owned()) {
        let _ = vm.run_code_obj(code, scope.clone());
    }
}

#[cfg(test)]
mod tests;
