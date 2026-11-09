use rustyline::{
    Context, Helper,
    completion::{Completer, FilenameCompleter, Pair},
    highlight::Highlighter,
    hint::{Hinter, HistoryHinter},
    validate::{ValidationContext, ValidationResult, Validator},
};

#[derive(Default)]
pub struct ShellHelper {
    files: FilenameCompleter,
    hints: HistoryHinter,
    pub manual_multiline: bool,
}
impl Helper for ShellHelper {}
impl Highlighter for ShellHelper {}
impl Hinter for ShellHelper {
    type Hint = String;
    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hints.hint(line, pos, ctx)
    }
}
impl Completer for ShellHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if line.starts_with(":load ") || line.starts_with(":reload ") {
            return self.files.complete(line, pos, ctx);
        }
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ',' || c == '=')
            .map_or(0, |i| i + 1);
        let prefix = &line[start..pos];
        let words = [
            ":bluetooth ",
            ":start",
            ":cancel ",
            ":actions",
            ":invoke ",
            ":config",
            ":configure ",
            ":lifecycle",
            "ble.adapters",
            "ble.scan",
            "ble.list",
            "ble.watch",
            "ble.open",
            "ble.uuid",
            "runtime.spawn",
            "runtime.join",
            "runtime.yield",
            "runtime.tasks",
            "runtime.modules",
            "buffer.string",
            "buffer.alloc",
            "buffer.integer",
            "buffer.float",
            "errors.info",
            "errors.raise",
            "log.info",
            "log.warn",
            "log.error",
            "log.debug",
            "events.subscribe",
            "config.get",
            "config.define",
            "ui.environment",
            "ui.on_environment",
            "i18n.locale",
            "i18n.t",
            "action.register",
            "lifecycle.on",
            "lifecycle.start",
            "storage.get",
            "storage.set",
            "mock.gatt",
            ":help",
            ":new ",
            ":use ",
            ":instances",
            ":devices",
            ":load ",
            ":reload ",
            ":tasks",
            ":stats",
            ":snapshot",
            ":events",
            ":pause",
            ":resume",
            ":stop",
            ":destroy",
            ":begin",
            ":end",
            ":quit",
            "device.list",
            "device.open",
            "buffer.hex",
            "buffer.from",
            "runtime.every",
            "runtime.cancel",
            "runtime.sleep",
            "runtime.now_ms",
            "state.publish",
            "mock.replay",
            "require",
            "print",
            "return",
            "function",
            "local",
        ];
        Ok((
            start,
            words
                .iter()
                .filter(|s| s.starts_with(prefix))
                .map(|s| Pair {
                    display: s.to_string(),
                    replacement: s.to_string(),
                })
                .collect(),
        ))
    }
}
impl Validator for ShellHelper {
    fn validate(&self, ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
        Ok(if self.manual_multiline || ctx.input().starts_with(':') {
            ValidationResult::Valid(None)
        } else {
            validate_lua(ctx.input())
        })
    }
}

fn validate_lua(input: &str) -> ValidationResult {
    if input.len() > crate::runtime::MAX_SOURCE {
        return ValidationResult::Invalid(Some("source exceeds 64 KiB".into()));
    }
    let lua = mlua::Lua::new();
    if lua.load(format!("return {input}")).into_function().is_ok() {
        return ValidationResult::Valid(None);
    }
    match lua.load(input).into_function() {
        Err(mlua::Error::SyntaxError {
            incomplete_input: true,
            ..
        }) => ValidationResult::Incomplete,
        _ => ValidationResult::Valid(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiline_checks_compile_without_executing() {
        assert!(matches!(
            validate_lua("function f(x)"),
            ValidationResult::Incomplete
        ));
        assert!(matches!(
            validate_lua("function f(x)\nreturn x*2\nend"),
            ValidationResult::Valid(_)
        ));
        assert!(matches!(
            validate_lua("{hello = 42}"),
            ValidationResult::Valid(_)
        ));
        assert!(matches!(
            validate_lua("while true do end"),
            ValidationResult::Valid(_)
        ));
        assert!(matches!(
            validate_lua("local x = )"),
            ValidationResult::Valid(_)
        ));
    }
}
