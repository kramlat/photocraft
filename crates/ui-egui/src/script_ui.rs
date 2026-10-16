//! Plugins › Script-Fu and Plugins › Python-Fu: the console windows and Run Script….
//!
//! The interpreters live outside this crate (the shell's `Services::script_run` and
//! `Services::python_run` supply them), so this module only draws the consoles and routes the
//! menu commands. Scripts run against the session on the UI thread; a runaway Scheme script stops
//! at the interpreter's step budget.

use egui::{Key, RichText, ScrollArea, TextEdit};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::{PhotocraftApp, widgets};

/// Runs Scheme source against a session: `(printed output, value)` or the error message.
pub type ScriptRunFn = Box<dyn FnMut(&mut photocraft_engine::Session, &str) -> Result<(String, String), String>>;

/// Which scripting language a console speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Scheme,
    Python,
}

impl Lang {
    fn menu_prefix(self) -> &'static str {
        match self {
            Lang::Scheme => "plugins.scriptFu.",
            Lang::Python => "plugins.pythonFu.",
        }
    }
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Lang::Scheme => &["scm", "script"],
            Lang::Python => &["py"],
        }
    }
}

fn console(app: &mut PhotocraftApp, lang: Lang) -> &mut Console {
    match lang {
        Lang::Scheme => &mut app.script,
        Lang::Python => &mut app.python,
    }
}

/// Console state: the log, the input line and the recalled history.
#[derive(Default)]
pub struct Console {
    pub open: bool,
    input: String,
    log: Vec<Line>,
    history: Vec<String>,
    recall: Option<usize>,
}

enum Line {
    Input(String),
    Output(String),
    Value(String),
    Error(String),
}

const MAX_LOG: usize = 500;

impl Console {
    fn push(&mut self, line: Line) {
        self.log.push(line);
        if self.log.len() > MAX_LOG {
            let drop = self.log.len() - MAX_LOG;
            self.log.drain(..drop);
        }
    }
}

/// Runs `source` and logs the outcome in the console.
pub fn run_source(app: &mut PhotocraftApp, lang: Lang, source: &str) {
    let slot = match lang {
        Lang::Scheme => &mut app.services.script_run,
        Lang::Python => &mut app.services.python_run,
    };
    let Some(mut runner) = slot.take() else {
        let msg = match lang {
            Lang::Scheme => tl!("Script-Fu is not available in this build."),
            Lang::Python => tl!("Python-Fu is not available in this build."),
        };
        console(app, lang).push(Line::Error(msg.into()));
        return;
    };
    let result = runner(&mut app.session, source);
    match lang {
        Lang::Scheme => app.services.script_run = Some(runner),
        Lang::Python => app.services.python_run = Some(runner),
    }
    match result {
        Ok((output, value)) => {
            let c = console(app, lang);
            if !output.is_empty() {
                c.push(Line::Output(output.trim_end_matches('\n').to_string()));
            }
            if !value.is_empty() {
                c.push(Line::Value(value));
            }
            app.ui.status_error = false;
        }
        Err(e) => {
            console(app, lang).push(Line::Error(e.clone()));
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

/// `plugins.scriptFu.*` and `plugins.pythonFu.*` menu commands.
pub fn invoke(app: &mut PhotocraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let lang = [Lang::Scheme, Lang::Python].into_iter().find(|l| id.starts_with(l.menu_prefix()))?;
    match id.strip_prefix(lang.menu_prefix())? {
        "console" => {
            let c = console(app, lang);
            c.open = !c.open;
            Some(Ok(json!({"open": c.open})))
        }
        "runScript" => {
            if let Some(src) = params.get("source").and_then(Value::as_str) {
                run_source(app, lang, src);
                return Some(Ok(Value::Null));
            }
            let r = app.pick_file_bytes_filtered(lang.extensions(), move |app, name, bytes| {
                let src = String::from_utf8(bytes).map_err(|_| crate::i18n::fmt(tl!("{name} is not a text file."), &[("name", &name)]))?;
                let c = console(app, lang);
                c.open = true;
                c.push(Line::Input(format!("# {name}")));
                run_source(app, lang, &src);
                Ok(Value::Null)
            });
            Some(r)
        }
        _ => None,
    }
}

/// Both consoles.
pub fn windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    window(app, ctx, Lang::Scheme);
    window(app, ctx, Lang::Python);
}

fn window(app: &mut PhotocraftApp, ctx: &egui::Context, lang: Lang) {
    if !console(app, lang).open {
        return;
    }
    let (title, hint, key) = match lang {
        Lang::Scheme => (tl!("Script-Fu Console"), tl!("Scheme expression"), "script-fu-console"),
        Lang::Python => (tl!("Python-Fu Console"), tl!("Python statement"), "python-fu-console"),
    };
    let mut open = true;
    let t = Tokens::get(ctx);
    let mut submit: Option<String> = None;
    egui::Window::new(title).id(egui::Id::new(key)).open(&mut open).default_size([460.0, 320.0]).resizable(true).show(ctx, |ui| {
        let c = console(app, lang);
        let input_height = 28.0;
        ScrollArea::vertical().stick_to_bottom(true).max_height((ui.available_height() - input_height - 8.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
            for line in &c.log {
                let (text, color) = match line {
                    Line::Input(s) => (format!("> {s}"), t.text_dim),
                    Line::Output(s) => (s.clone(), t.text),
                    Line::Value(s) => (format!("=> {s}"), t.accent),
                    Line::Error(s) => (s.clone(), egui::Color32::from_rgb(220, 80, 80)),
                };
                ui.label(RichText::new(text).monospace().color(color));
            }
        });
        ui.horizontal(|ui| {
            let id = ui.id().with("script-input");
            let edit = TextEdit::singleline(&mut c.input).id(id).font(egui::TextStyle::Monospace).hint_text(hint).desired_width(ui.available_width() - 70.0);
            let response = ui.add(edit);
            if response.has_focus() {
                let up = ui.input(|i| i.key_pressed(Key::ArrowUp));
                let down = ui.input(|i| i.key_pressed(Key::ArrowDown));
                if (up || down) && !c.history.is_empty() {
                    let last = c.history.len() - 1;
                    let next = match (c.recall, up) {
                        (None, true) => Some(last),
                        (Some(i), true) => Some(i.saturating_sub(1)),
                        (Some(i), false) if i < last => Some(i + 1),
                        _ => None,
                    };
                    c.recall = next;
                    c.input = next.and_then(|i| c.history.get(i).cloned()).unwrap_or_default();
                }
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if enter && !c.input.trim().is_empty() {
                submit = Some(std::mem::take(&mut c.input));
                response.request_focus();
            }
            if widgets::secondary_button(ui, tl!("Clear"), 0.0).clicked() {
                c.log.clear();
            }
        });
    });
    if let Some(src) = submit {
        let c = console(app, lang);
        c.recall = None;
        c.history.push(src.clone());
        c.push(Line::Input(src.clone()));
        run_source(app, lang, &src);
    }
    if !open {
        console(app, lang).open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;

    fn app_with_runner() -> PhotocraftApp {
        let services = Services {
            script_run: Some(Box::new(|session, src| {
                if src.contains("boom") {
                    return Err("boom".into());
                }
                let docs = session.documents().len();
                Ok((format!("saw {src}"), docs.to_string()))
            })),
            ..Default::default()
        };
        PhotocraftApp::new(photocraft_engine::Session::new(), services)
    }

    #[test]
    fn the_console_menu_command_toggles_the_window() {
        let mut app = app_with_runner();
        assert!(!app.script.open);
        invoke(&mut app, "plugins.scriptFu.console", &Value::Null).unwrap().unwrap();
        assert!(app.script.open);
        invoke(&mut app, "plugins.scriptFu.console", &Value::Null).unwrap().unwrap();
        assert!(!app.script.open);
        assert!(invoke(&mut app, "plugins.other", &Value::Null).is_none());
    }

    #[test]
    fn running_source_logs_output_value_and_errors() {
        let mut app = app_with_runner();
        invoke(&mut app, "plugins.scriptFu.runScript", &json!({"source": "(+ 1 2)"})).unwrap().unwrap();
        assert!(matches!(app.script.log.as_slice(), [Line::Output(o), Line::Value(v)] if o == "saw (+ 1 2)" && v == "0"));
        run_source(&mut app, Lang::Scheme, "(boom)");
        assert!(matches!(app.script.log.last(), Some(Line::Error(e)) if e == "boom"));
        assert!(app.ui.status_error);
        assert!(app.services.script_run.is_some(), "the runner is put back");
    }

    #[test]
    fn without_a_runner_the_console_says_so() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Services::default());
        run_source(&mut app, Lang::Scheme, "(+ 1 2)");
        assert!(matches!(app.script.log.last(), Some(Line::Error(_))));
    }

    #[test]
    fn the_window_draws_with_a_log() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 600.0)).build_eframe(|cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, Default::default());
            let mut app = app_with_runner();
            app.script.open = true;
            run_source(&mut app, Lang::Scheme, "(+ 1 2)");
            app
        });
        h.run_steps(3);
        assert!(h.state().script.open, "the console stays open");
        h.state_mut().script.open = false;
        h.run_steps(2);
        assert!(!h.state().script.open);
    }
}
