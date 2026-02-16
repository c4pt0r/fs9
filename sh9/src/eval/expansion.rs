use crate::ast::*;
use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;
use super::{ExecContext, Output};
use super::utils::{contains_glob_chars, match_glob_pattern};

impl Shell {
    pub async fn expand_word(&mut self, word: &Word, ctx: &mut ExecContext) -> Sh9Result<String> {
        let mut result = String::new();
        
        for part in &word.parts {
            match part {
                WordPart::Literal(s) => {
                    // Tilde expansion: ~ or ~/path → $HOME or $HOME/path
                    let tilde_expanded = self.expand_tilde(s, ctx);
                    let expanded = self.expand_variables_in_string(&tilde_expanded, ctx).await?;
                    result.push_str(&expanded);
                }
                WordPart::SingleQuoted(s) => {
                    result.push_str(s);
                }
                WordPart::DoubleQuoted(s) => {
                    // Double-quoted: expand variables but NOT tilde
                    let expanded = self.expand_variables_in_string(s, ctx).await?;
                    result.push_str(&expanded);
                }
                WordPart::Variable(name) => {
                    let value = self.get_variable_value(name, ctx);
                    result.push_str(&value);
                }
                WordPart::BracedVariable(name) => {
                    let value = self.get_variable_value(name, ctx);
                    result.push_str(&value);
                }
                WordPart::Arithmetic(expr) => {
                    let value = self.evaluate_arithmetic(expr, ctx)?;
                    result.push_str(&value.to_string());
                }
                WordPart::CommandSub(cmd) => {
                    let output = self.execute_command_sub(cmd, ctx).await?;
                    result.push_str(&output);
                }
            }
        }
        
        Ok(result)
    }

    async fn expand_variables_in_string(&mut self, s: &str, ctx: &mut ExecContext) -> Sh9Result<String> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();
        
        while let Some(c) = chars.next() {
            if c == '$' {
                if chars.peek() == Some(&'(') {
                    chars.next();
                    if chars.peek() == Some(&'(') {
                        chars.next();
                        let expr = Self::collect_balanced_parens(&mut chars, 2);
                        let value = self.evaluate_arithmetic(&expr, ctx)?;
                        result.push_str(&value.to_string());
                    } else {
                        let cmd = Self::collect_balanced_parens(&mut chars, 1);
                        let output = self.execute_command_sub(&cmd, ctx).await?;
                        result.push_str(&output);
                    }
                } else if chars.peek() == Some(&'{') {
                    chars.next();
                    let mut content = String::new();
                    let mut depth = 1;
                    while let Some(&c) = chars.peek() {
                        if c == '{' {
                            depth += 1;
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                chars.next();
                                break;
                            }
                        }
                        content.push(c);
                        chars.next();
                    }
                    let expanded = Box::pin(self.expand_braced_param(&content, ctx)).await?;
                    result.push_str(&expanded);
                } else if chars.peek().map(|c| c.is_alphabetic() || *c == '_' || *c == '?').unwrap_or(false) {
                    let mut name = String::new();
                    let first_char = *chars.peek().unwrap();
                    if first_char == '?' {
                        name.push('?');
                        chars.next();
                    } else {
                        while let Some(&c) = chars.peek() {
                            if c.is_alphanumeric() || c == '_' {
                                name.push(c);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    let value = self.get_variable_value(&name, ctx);
                    result.push_str(&value);
                } else if chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    let first_char = chars.next().unwrap();
                    let name = first_char.to_string();
                    let value = self.get_variable_value(&name, ctx);
                    result.push_str(&value);
                } else {
                    result.push('$');
                }
            } else {
                result.push(c);
            }
        }
        
        Ok(result)
    }

    fn expand_tilde(&self, s: &str, ctx: &ExecContext) -> String {
        if s.starts_with("~/") {
            let home = self.get_variable_value("HOME", ctx);
            if home.is_empty() {
                s.to_string()
            } else {
                format!("{}{}", home, &s[1..])
            }
        } else if s == "~" {
            let home = self.get_variable_value("HOME", ctx);
            if home.is_empty() {
                s.to_string()
            } else {
                home
            }
        } else {
            s.to_string()
        }
    }

    async fn expand_braced_param(&mut self, content: &str, ctx: &mut ExecContext) -> Sh9Result<String> {
        // ${#var} — string length
        if let Some(var_name) = content.strip_prefix('#') {
            let value = self.get_variable_value(var_name, ctx);
            return Ok(value.len().to_string());
        }

        // Try to find an operator in the content
        // Check two-char operators first, then single-char
        let operators = [":-", "-", ":=", "=", ":+", "+", "##", "#", "%%", "%"];
        for op in &operators {
            if let Some(pos) = content.find(op) {
                // Make sure we're not matching inside the var name for # and %
                // For # and %, the var name is everything before the operator
                let var_name = &content[..pos];
                let operand = &content[pos + op.len()..];

                // Skip if var_name is empty (shouldn't happen)
                if var_name.is_empty() {
                    continue;
                }

                let value = self.get_variable_value(var_name, ctx);
                let is_set = !value.is_empty() || self.has_variable(var_name, ctx);

                match *op {
                    ":-" => {
                        // ${var:-word} — use word if var is unset or empty
                        return Ok(if value.is_empty() {
                            self.expand_variables_in_string(operand, ctx).await?
                        } else {
                            value
                        });
                    }
                    "-" => {
                        // ${var-word} — use word if var is unset
                        return Ok(if !is_set {
                            self.expand_variables_in_string(operand, ctx).await?
                        } else {
                            value
                        });
                    }
                    ":=" => {
                        // ${var:=word} — assign word if var is unset or empty
                        if value.is_empty() {
                            let default = self.expand_variables_in_string(operand, ctx).await?;
                            self.set_var(var_name, &default);
                            return Ok(default);
                        }
                        return Ok(value);
                    }
                    "=" => {
                        // ${var=word} — assign word if var is unset
                        if !is_set {
                            let default = self.expand_variables_in_string(operand, ctx).await?;
                            self.set_var(var_name, &default);
                            return Ok(default);
                        }
                        return Ok(value);
                    }
                    ":+" => {
                        // ${var:+word} — use word if var is set and non-empty
                        return Ok(if !value.is_empty() {
                            self.expand_variables_in_string(operand, ctx).await?
                        } else {
                            String::new()
                        });
                    }
                    "+" => {
                        // ${var+word} — use word if var is set
                        return Ok(if is_set {
                            self.expand_variables_in_string(operand, ctx).await?
                        } else {
                            String::new()
                        });
                    }
                    "##" => {
                        // ${var##pattern} — remove longest prefix match
                        return Ok(Self::remove_prefix(&value, operand, true));
                    }
                    "#" => {
                        // ${var#pattern} — remove shortest prefix match
                        return Ok(Self::remove_prefix(&value, operand, false));
                    }
                    "%%" => {
                        // ${var%%pattern} — remove longest suffix match
                        return Ok(Self::remove_suffix(&value, operand, true));
                    }
                    "%" => {
                        // ${var%pattern} — remove shortest suffix match
                        return Ok(Self::remove_suffix(&value, operand, false));
                    }
                    _ => unreachable!(),
                }
            }
        }

        // No operator found — simple variable reference
        Ok(self.get_variable_value(content, ctx))
    }

    fn has_variable(&self, name: &str, ctx: &ExecContext) -> bool {
        match name {
            "?" | "0" | "PWD" => return true,
            _ => {}
        }
        if let Ok(n) = name.parse::<usize>() {
            return n > 0 && n <= ctx.positional.len();
        }
        ctx.locals.contains_key(name)
            || self.get_var(name).is_some()
            || std::env::var(name).is_ok()
    }

    fn remove_prefix(value: &str, pattern: &str, greedy: bool) -> String {
        use super::utils::match_glob_pattern;
        if greedy {
            // Remove longest prefix: try from the longest to shortest
            for i in (0..=value.len()).rev() {
                if match_glob_pattern(pattern, &value[..i]) {
                    return value[i..].to_string();
                }
            }
        } else {
            // Remove shortest prefix: try from shortest to longest
            for i in 0..=value.len() {
                if match_glob_pattern(pattern, &value[..i]) {
                    return value[i..].to_string();
                }
            }
        }
        value.to_string()
    }

    fn remove_suffix(value: &str, pattern: &str, greedy: bool) -> String {
        use super::utils::match_glob_pattern;
        if greedy {
            // Remove longest suffix: try from the longest to shortest
            for i in 0..=value.len() {
                if match_glob_pattern(pattern, &value[i..]) {
                    return value[..i].to_string();
                }
            }
        } else {
            // Remove shortest suffix: try from shortest to longest
            for i in (0..=value.len()).rev() {
                if match_glob_pattern(pattern, &value[i..]) {
                    return value[..i].to_string();
                }
            }
        }
        value.to_string()
    }

    pub(crate) fn collect_balanced_parens(chars: &mut std::iter::Peekable<std::str::Chars>, initial_depth: usize) -> String {
        let mut result = String::new();
        let mut depth = initial_depth;
        
        for c in chars.by_ref() {
            if c == '(' {
                depth += 1;
                result.push(c);
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                result.push(c);
            } else {
                result.push(c);
            }
        }
        
        result
    }

    pub(crate) fn get_variable_value(&self, name: &str, ctx: &ExecContext) -> String {
        match name {
            "?" => return self.last_exit_code.to_string(),
            "0" => return "sh9".to_string(),
            "PWD" => return self.cwd.clone(),
            _ => {}
        }
        
        if let Ok(n) = name.parse::<usize>() {
            if n > 0 && n <= ctx.positional.len() {
                return ctx.positional[n - 1].clone();
            }
            return String::new();
        }
        
        if let Some(value) = ctx.locals.get(name) {
            return value.clone();
        }
        
        if let Some(value) = self.get_var(name) {
            return value.to_string();
        }
        
        if let Ok(value) = std::env::var(name) {
            return value;
        }
        
        String::new()
    }

    async fn execute_command_sub(&mut self, cmd: &str, ctx: &mut ExecContext) -> Sh9Result<String> {
        use crate::parser::parse;
        
        let script = parse(cmd).map_err(|e| {
            Sh9Error::Parse(format!("Command substitution parse error: {:?}", e))
        })?;
        
        let saved_stdout = std::mem::replace(&mut ctx.stdout, Output::Buffer(Vec::new()));
        
        for stmt in &script.statements {
            self.execute_statement_boxed(stmt, ctx).await?;
        }
        
        let output = if let Output::Buffer(buf) = std::mem::replace(&mut ctx.stdout, saved_stdout) {
            String::from_utf8_lossy(&buf).trim_end_matches('\n').to_string()
        } else {
            String::new()
        };
        
        Ok(output)
    }

    pub(crate) async fn expand_glob(&self, pattern: &str) -> Vec<String> {
        if !contains_glob_chars(pattern) {
            return vec![pattern.to_string()];
        }
        
        let (dir, file_pattern) = if pattern.contains('/') {
            let last_slash = pattern.rfind('/').unwrap();
            let dir_part = &pattern[..=last_slash];
            let file_part = &pattern[last_slash + 1..];
            
            if contains_glob_chars(dir_part) {
                return vec![pattern.to_string()];
            }
            
            (self.resolve_path(dir_part.trim_end_matches('/')), file_part.to_string())
        } else {
            (self.cwd.clone(), pattern.to_string())
        };
        
        let router = self.router();

        let entries = match router.readdir(&dir).await {
            Ok(e) => e,
            Err(_) => return vec![pattern.to_string()],
        };
        
        let mut matches: Vec<String> = entries
            .iter()
            .filter(|e| match_glob_pattern(&file_pattern, &e.name))
            .map(|e| {
                if pattern.contains('/') {
                    let last_slash = pattern.rfind('/').unwrap();
                    format!("{}{}", &pattern[..=last_slash], e.name)
                } else {
                    e.name.clone()
                }
            })
            .collect();
        
        matches.sort();
        
        if matches.is_empty() {
            vec![pattern.to_string()]
        } else {
            matches
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::Shell;
    use crate::eval::namespace::MountFlags;

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "{}_{}_{}",
                prefix,
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("failed to create temp test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn expand_glob_uses_local_mount_for_matching_files() {
        let tmp = TempDirGuard::new("sh9_glob_local");
        fs::write(tmp.path().join("a.txt"), b"a").expect("write failed");
        fs::write(tmp.path().join("b.txt"), b"b").expect("write failed");
        fs::write(tmp.path().join("c.log"), b"c").expect("write failed");

        let shell = Shell::new("http://localhost:8080");
        {
            let mut ns = shell.namespace.write().unwrap();
            ns.bind(tmp.path(), "/mnt", MountFlags::MREPL);
        }

        let txt = shell.expand_glob("/mnt/*.txt").await;
        assert_eq!(txt, vec!["/mnt/a.txt", "/mnt/b.txt"]);

        let all = shell.expand_glob("/mnt/*").await;
        assert_eq!(all, vec!["/mnt/a.txt", "/mnt/b.txt", "/mnt/c.log"]);
    }

    #[tokio::test]
    async fn expand_glob_union_mount_merges_and_deduplicates() {
        let lower = TempDirGuard::new("sh9_glob_union_lower");
        let upper = TempDirGuard::new("sh9_glob_union_upper");
        fs::write(lower.path().join("a.txt"), b"a").expect("write failed");
        fs::write(lower.path().join("shared.txt"), b"lower").expect("write failed");
        fs::write(upper.path().join("b.txt"), b"b").expect("write failed");
        fs::write(upper.path().join("shared.txt"), b"upper").expect("write failed");

        let shell = Shell::new("http://localhost:8080");
        {
            let mut ns = shell.namespace.write().unwrap();
            ns.bind(lower.path(), "/union", MountFlags::MREPL);
            ns.bind(upper.path(), "/union", MountFlags::MBEFORE);
        }

        let matches = shell.expand_glob("/union/*.txt").await;
        assert_eq!(
            matches,
            vec!["/union/a.txt", "/union/b.txt", "/union/shared.txt"]
        );
    }

    #[tokio::test]
    async fn expand_glob_returns_pattern_when_no_match_or_no_client() {
        let tmp = TempDirGuard::new("sh9_glob_nomatch");
        fs::write(tmp.path().join("a.txt"), b"a").expect("write failed");

        let shell = Shell::new("http://localhost:8080");
        {
            let mut ns = shell.namespace.write().unwrap();
            ns.bind(tmp.path(), "/mnt", MountFlags::MREPL);
        }

        let no_match = shell.expand_glob("/mnt/*.xyz").await;
        assert_eq!(no_match, vec!["/mnt/*.xyz"]);

        let unmounted = shell.expand_glob("/remote/*.txt").await;
        assert_eq!(unmounted, vec!["/remote/*.txt"]);
    }
}
