//! The evaluator: special forms, procedure application and proper tail calls.

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{Closure, Env, Value, assign, eqv, lookup, new_env};
use crate::{Error, Interp, Result};

enum Step {
    Done(Value),
    /// Continue with this expression in this environment (a tail position).
    Tail(Value, Env),
}

fn syntax(form: &str) -> Error {
    Error::msg(format!("bad syntax in {form}"))
}

fn sym_name(v: &Value) -> Option<Rc<str>> {
    match v {
        Value::Sym(s) => Some(s.clone()),
        _ => None,
    }
}

/// `(a b . rest)` / `(a b)` / `rest` parameter lists.
type ParamList = (Vec<Rc<str>>, Option<Rc<str>>);

fn parse_params(spec: &Value) -> Result<ParamList> {
    let mut params = Vec::new();
    let mut cur = spec.clone();
    loop {
        match cur {
            Value::Nil => return Ok((params, None)),
            Value::Sym(s) => return Ok((params, Some(s))),
            Value::Pair(p) => {
                params.push(sym_name(&p.car.borrow()).ok_or_else(|| syntax("lambda parameters"))?);
                let next = p.cdr.borrow().clone();
                cur = next;
            }
            _ => return Err(syntax("lambda parameters")),
        }
    }
}

fn make_closure(spec: &Value, body: Vec<Value>, env: &Env, name: Option<Rc<str>>) -> Result<Value> {
    let (params, rest) = parse_params(spec)?;
    if body.is_empty() {
        return Err(syntax("lambda (empty body)"));
    }
    Ok(Value::Closure(Rc::new(Closure { params, rest, body, env: env.clone(), name: RefCell::new(name) })))
}

impl Interp {
    pub(crate) fn tick(&mut self) -> Result<()> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(Error::msg("script exceeded its step budget"));
        }
        Ok(())
    }

    /// Evaluates `expr` in `env`.
    pub fn eval(&mut self, expr: &Value, env: &Env) -> Result<Value> {
        if self.depth >= self.max_depth {
            return Err(Error::msg("recursion too deep"));
        }
        self.depth += 1;
        let r = self.eval_inner(expr, env);
        self.depth -= 1;
        r
    }

    fn eval_inner(&mut self, expr: &Value, env: &Env) -> Result<Value> {
        let mut expr = expr.clone();
        let mut env = env.clone();
        loop {
            self.tick()?;
            match &expr {
                Value::Sym(s) => return self.variable(s, &env),
                Value::Pair(p) => {
                    let head = p.car.borrow().clone();
                    if let Value::Sym(name) = &head
                        && let Some(step) = self.special(name, &expr, &env)?
                    {
                        match step {
                            Step::Done(v) => return Ok(v),
                            Step::Tail(e, en) => {
                                expr = e;
                                env = en;
                                continue;
                            }
                        }
                    }
                    let rest = p.cdr.borrow().clone();
                    let mut args = Vec::new();
                    for a in rest.to_vec()? {
                        args.push(self.eval(&a, &env)?);
                    }
                    // A symbol that names no variable may be a host procedure (Script-Fu's PDB).
                    if let Value::Sym(name) = &head
                        && lookup(&env, name).is_none()
                    {
                        match self.host_call(name, &args) {
                            Some(r) => return r,
                            None => return Err(Error::msg(format!("unbound variable: {name}"))),
                        }
                    }
                    let f = self.eval(&head, &env)?;
                    match f {
                        Value::Closure(c) => {
                            let call_env = self.bind(&c, args)?;
                            let (last, init) = c.body.split_last().ok_or_else(|| syntax("procedure body"))?;
                            for form in init {
                                self.eval(form, &call_env)?;
                            }
                            expr = last.clone();
                            env = call_env;
                        }
                        other => return self.apply_value(&other, args),
                    }
                }
                other => return Ok(other.clone()),
            }
        }
    }

    fn variable(&mut self, name: &str, env: &Env) -> Result<Value> {
        if let Some(v) = lookup(env, name) {
            return Ok(v);
        }
        match name {
            "TRUE" => return Ok(Value::Bool(true)),
            "FALSE" => return Ok(Value::Bool(false)),
            _ => {}
        }
        if let Some(mut host) = self.host.take() {
            let v = host.constant(name);
            self.host = Some(host);
            if let Some(v) = v {
                return Ok(v);
            }
        }
        Err(Error::msg(format!("unbound variable: {name}")))
    }

    fn host_call(&mut self, name: &str, args: &[Value]) -> Option<Result<Value>> {
        let mut host = self.host.take()?;
        let r = host.call(self, name, args);
        self.host = Some(host);
        r
    }

    fn bind(&mut self, c: &Closure, args: Vec<Value>) -> Result<Env> {
        if args.len() < c.params.len() || (c.rest.is_none() && args.len() > c.params.len()) {
            let name = c.name.borrow().clone().unwrap_or_else(|| "procedure".into());
            return Err(Error::msg(format!("{name}: expected {}{} arguments, got {}", c.params.len(), if c.rest.is_some() { "+" } else { "" }, args.len())));
        }
        let env = new_env(Some(c.env.clone()));
        {
            let mut vars = env.vars.borrow_mut();
            let mut it = args.into_iter();
            for p in &c.params {
                vars.insert(p.clone(), it.next().unwrap_or(Value::Nil));
            }
            if let Some(r) = &c.rest {
                vars.insert(r.clone(), Value::list(it.collect::<Vec<_>>()));
            }
        }
        Ok(env)
    }

    /// Calls a procedure value with evaluated arguments.
    pub fn apply_value(&mut self, f: &Value, args: Vec<Value>) -> Result<Value> {
        match f {
            Value::Builtin(b) => {
                if args.len() < b.min || b.max.is_some_and(|m| args.len() > m) {
                    return Err(Error::msg(format!("{}: wrong number of arguments ({})", b.name, args.len())));
                }
                (b.f)(self, &args)
            }
            Value::Closure(c) => {
                if self.depth >= self.max_depth {
                    return Err(Error::msg("recursion too deep"));
                }
                self.depth += 1;
                let r = (|| {
                    let env = self.bind(c, args)?;
                    let mut last = Value::Unspecified;
                    for form in &c.body {
                        last = self.eval(form, &env)?;
                    }
                    Ok(last)
                })();
                self.depth -= 1;
                r
            }
            other => Err(Error::msg(format!("not a procedure: {}", other.write()))),
        }
    }

    /// Special forms. `None` when `name` isn't one.
    fn special(&mut self, name: &str, form: &Value, env: &Env) -> Result<Option<Step>> {
        const FORMS: &[&str] = &[
            "quote", "quasiquote", "if", "define", "set!", "lambda", "let", "let*", "letrec", "letrec*", "begin", "cond", "case", "and", "or", "when",
            "unless", "do", "assert",
        ];
        if !FORMS.contains(&name) || lookup(env, name).is_some() {
            return Ok(None);
        }
        let items = form.to_vec()?;
        let arg = |i: usize| items.get(i).ok_or_else(|| syntax(name));
        let body_from = |i: usize| items.get(i..).unwrap_or_default().to_vec();
        Ok(Some(match name {
            "quote" => Step::Done(arg(1)?.clone()),
            "quasiquote" => Step::Done(self.quasi(arg(1)?, env, 1)?),
            "if" => {
                let test = self.eval(arg(1)?, env)?;
                if test.truthy() {
                    Step::Tail(arg(2)?.clone(), env.clone())
                } else if let Some(alt) = items.get(3) {
                    Step::Tail(alt.clone(), env.clone())
                } else {
                    Step::Done(Value::Unspecified)
                }
            }
            "define" => {
                let target = arg(1)?;
                match target {
                    Value::Sym(s) => {
                        let v = match items.get(2) {
                            Some(e) => self.eval(e, env)?,
                            None => Value::Unspecified,
                        };
                        if let Value::Closure(c) = &v
                            && c.name.borrow().is_none()
                        {
                            *c.name.borrow_mut() = Some(s.clone());
                        }
                        env.vars.borrow_mut().insert(s.clone(), v);
                        Step::Done(Value::Sym(s.clone()))
                    }
                    Value::Pair(p) => {
                        let fname = sym_name(&p.car.borrow()).ok_or_else(|| syntax("define"))?;
                        let params = p.cdr.borrow().clone();
                        let c = make_closure(&params, body_from(2), env, Some(fname.clone()))?;
                        env.vars.borrow_mut().insert(fname.clone(), c);
                        Step::Done(Value::Sym(fname))
                    }
                    _ => return Err(syntax("define")),
                }
            }
            "set!" => {
                let n = sym_name(arg(1)?).ok_or_else(|| syntax("set!"))?;
                let v = self.eval(arg(2)?, env)?;
                if !assign(env, &n, v) {
                    return Err(Error::msg(format!("set!: unbound variable {n}")));
                }
                Step::Done(Value::Unspecified)
            }
            "lambda" => Step::Done(make_closure(arg(1)?, body_from(2), env, None)?),
            "begin" => self.sequence(&body_from(1), env)?,
            "let" => {
                // Named let: (let name ((v init) …) body…)
                if let Some(loop_name) = sym_name(arg(1)?) {
                    let (vars, inits) = self.bindings(arg(2)?, env)?;
                    let frame = new_env(Some(env.clone()));
                    let params = Value::list(vars.iter().map(|v| Value::Sym(v.clone())).collect::<Vec<_>>());
                    let c = make_closure(&params, body_from(3), &frame, Some(loop_name.clone()))?;
                    frame.vars.borrow_mut().insert(loop_name, c.clone());
                    let call_env = match &c {
                        Value::Closure(cl) => self.bind(cl, inits)?,
                        _ => return Err(syntax("let")),
                    };
                    self.sequence(&match &c {
                        Value::Closure(cl) => cl.body.clone(),
                        _ => Vec::new(),
                    }, &call_env)?
                } else {
                    let (vars, inits) = self.bindings(arg(1)?, env)?;
                    let frame = new_env(Some(env.clone()));
                    for (v, i) in vars.into_iter().zip(inits) {
                        frame.vars.borrow_mut().insert(v, i);
                    }
                    self.sequence(&body_from(2), &frame)?
                }
            }
            "let*" => {
                let mut frame = new_env(Some(env.clone()));
                for b in arg(1)?.to_vec()? {
                    let parts = b.to_vec()?;
                    let n = parts.first().and_then(sym_name).ok_or_else(|| syntax("let*"))?;
                    let v = match parts.get(1) {
                        Some(e) => self.eval(e, &frame)?,
                        None => Value::Unspecified,
                    };
                    frame = new_env(Some(frame));
                    frame.vars.borrow_mut().insert(n, v);
                }
                self.sequence(&body_from(2), &frame)?
            }
            "letrec" | "letrec*" => {
                let frame = new_env(Some(env.clone()));
                for b in arg(1)?.to_vec()? {
                    let parts = b.to_vec()?;
                    let n = parts.first().and_then(sym_name).ok_or_else(|| syntax("letrec"))?;
                    let v = match parts.get(1) {
                        Some(e) => self.eval(e, &frame)?,
                        None => Value::Unspecified,
                    };
                    if let Value::Closure(c) = &v
                        && c.name.borrow().is_none()
                    {
                        *c.name.borrow_mut() = Some(n.clone());
                    }
                    frame.vars.borrow_mut().insert(n, v);
                }
                self.sequence(&body_from(2), &frame)?
            }
            "cond" => {
                for clause in body_from(1) {
                    let parts = clause.to_vec()?;
                    let test = parts.first().ok_or_else(|| syntax("cond"))?;
                    let is_else = test.as_sym() == Some("else");
                    let value = if is_else { Value::Bool(true) } else { self.eval(test, env)? };
                    if value.truthy() {
                        if parts.len() == 1 {
                            return Ok(Some(Step::Done(value)));
                        }
                        if parts.get(1).and_then(Value::as_sym) == Some("=>") {
                            let f = self.eval(parts.get(2).ok_or_else(|| syntax("cond"))?, env)?;
                            return Ok(Some(Step::Done(self.apply_value(&f, vec![value])?)));
                        }
                        return Ok(Some(self.sequence(&parts[1..], env)?));
                    }
                }
                Step::Done(Value::Unspecified)
            }
            "case" => {
                let key = self.eval(arg(1)?, env)?;
                for clause in body_from(2) {
                    let parts = clause.to_vec()?;
                    let head = parts.first().ok_or_else(|| syntax("case"))?;
                    let hit = head.as_sym() == Some("else") || head.to_vec()?.iter().any(|d| eqv(d, &key));
                    if hit {
                        return Ok(Some(self.sequence(&parts[1..], env)?));
                    }
                }
                Step::Done(Value::Unspecified)
            }
            "and" => {
                let forms = body_from(1);
                let Some((last, init)) = forms.split_last() else { return Ok(Some(Step::Done(Value::Bool(true)))) };
                for f in init {
                    if !self.eval(f, env)?.truthy() {
                        return Ok(Some(Step::Done(Value::Bool(false))));
                    }
                }
                Step::Tail(last.clone(), env.clone())
            }
            "or" => {
                let forms = body_from(1);
                let Some((last, init)) = forms.split_last() else { return Ok(Some(Step::Done(Value::Bool(false)))) };
                for f in init {
                    let v = self.eval(f, env)?;
                    if v.truthy() {
                        return Ok(Some(Step::Done(v)));
                    }
                }
                Step::Tail(last.clone(), env.clone())
            }
            "when" | "unless" => {
                let t = self.eval(arg(1)?, env)?.truthy();
                if t == (name == "when") { self.sequence(&body_from(2), env)? } else { Step::Done(Value::Unspecified) }
            }
            "do" => {
                // (do ((var init step)…) (test result…) body…)
                let specs: Vec<Vec<Value>> = arg(1)?.to_vec()?.iter().map(Value::to_vec).collect::<Result<_>>()?;
                let end = arg(2)?.to_vec()?;
                let mut frame = new_env(Some(env.clone()));
                for s in &specs {
                    let n = s.first().and_then(sym_name).ok_or_else(|| syntax("do"))?;
                    let v = self.eval(s.get(1).ok_or_else(|| syntax("do"))?, env)?;
                    frame.vars.borrow_mut().insert(n, v);
                }
                loop {
                    self.tick()?;
                    if self.eval(end.first().ok_or_else(|| syntax("do"))?, &frame)?.truthy() {
                        return Ok(Some(self.sequence(end.get(1..).unwrap_or_default(), &frame)?));
                    }
                    for b in body_from(3) {
                        self.eval(&b, &frame)?;
                    }
                    let next = new_env(Some(env.clone()));
                    for s in &specs {
                        let n = s.first().and_then(sym_name).ok_or_else(|| syntax("do"))?;
                        let v = match s.get(2) {
                            Some(step) => self.eval(step, &frame)?,
                            None => lookup(&frame, &n).unwrap_or(Value::Nil),
                        };
                        next.vars.borrow_mut().insert(n, v);
                    }
                    frame = next;
                }
            }
            "assert" => {
                if self.eval(arg(1)?, env)?.truthy() {
                    Step::Done(Value::Unspecified)
                } else {
                    return Err(Error::msg(format!("assertion failed: {}", arg(1)?.write())));
                }
            }
            _ => return Ok(None),
        }))
    }

    /// Evaluates all but the last form; the last is a tail position.
    fn sequence(&mut self, forms: &[Value], env: &Env) -> Result<Step> {
        let Some((last, init)) = forms.split_last() else { return Ok(Step::Done(Value::Unspecified)) };
        for f in init {
            self.eval(f, env)?;
        }
        Ok(Step::Tail(last.clone(), env.clone()))
    }

    /// `((name init) …)` as names and evaluated inits.
    fn bindings(&mut self, spec: &Value, env: &Env) -> Result<(Vec<Rc<str>>, Vec<Value>)> {
        let (mut names, mut values) = (Vec::new(), Vec::new());
        for b in spec.to_vec()? {
            match &b {
                Value::Sym(s) => {
                    names.push(s.clone());
                    values.push(Value::Unspecified);
                }
                _ => {
                    let parts = b.to_vec()?;
                    names.push(parts.first().and_then(sym_name).ok_or_else(|| syntax("let"))?);
                    values.push(match parts.get(1) {
                        Some(e) => self.eval(e, env)?,
                        None => Value::Unspecified,
                    });
                }
            }
        }
        Ok((names, values))
    }

    /// Quasiquote expansion with `unquote` / `unquote-splicing` (nested levels counted).
    fn quasi(&mut self, t: &Value, env: &Env, depth: usize) -> Result<Value> {
        match t {
            Value::Pair(p) => {
                let head = p.car.borrow().clone();
                match head.as_sym() {
                    Some("unquote") => {
                        let inner = t.to_vec()?.get(1).cloned().ok_or_else(|| syntax("unquote"))?;
                        return if depth == 1 { self.eval(&inner, env) } else { Ok(Value::list([head, self.quasi(&inner, env, depth - 1)?])) };
                    }
                    Some("quasiquote") => {
                        let inner = t.to_vec()?.get(1).cloned().ok_or_else(|| syntax("quasiquote"))?;
                        return Ok(Value::list([head, self.quasi(&inner, env, depth + 1)?]));
                    }
                    _ => {}
                }
                if let Value::Pair(hp) = &head
                    && hp.car.borrow().as_sym() == Some("unquote-splicing")
                    && depth == 1
                {
                    let inner = head.to_vec()?.get(1).cloned().ok_or_else(|| syntax("unquote-splicing"))?;
                    let spliced = self.eval(&inner, env)?.to_vec()?;
                    let rest = self.quasi(&p.cdr.borrow().clone(), env, depth)?;
                    return Ok(spliced.into_iter().rev().fold(rest, |tail, v| Value::cons(v, tail)));
                }
                let a = self.quasi(&head, env, depth)?;
                let d = self.quasi(&p.cdr.borrow().clone(), env, depth)?;
                Ok(Value::cons(a, d))
            }
            Value::Vector(items) => {
                let list = Value::list(items.borrow().clone());
                Ok(Value::vector(self.quasi(&list, env, depth)?.to_vec()?))
            }
            other => Ok(other.clone()),
        }
    }
}

