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
                    let expanded = self.expand_variables_in_string(s, ctx).await?;
                    result.push_str(&expanded);
                }
                WordPart::SingleQuoted(s) => {
                    result.push_str(s);
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
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '}' {
                            chars.next();
                            break;
                        }
                        name.push(c);
                        chars.next();
                    }
                    let value = self.get_variable_value(&name, ctx);
                    result.push_str(&value);
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

    pub(crate) fn collect_balanced_parens(chars: &mut std::iter::Peekable<std::str::Chars>, initial_depth: usize) -> String {
        let mut result = String::new();
        let mut depth = initial_depth;
        
        while let Some(c) = chars.next() {
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
        
        let client = match &self.client {
            Some(c) => c,
            None => return vec![pattern.to_string()],
        };
        
        let entries = match client.readdir(&dir).await {
            Ok(e) => e,
            Err(_) => return vec![pattern.to_string()],
        };
        
        let mut matches: Vec<String> = entries
            .iter()
            .filter(|e| match_glob_pattern(&file_pattern, e.name()))
            .map(|e| {
                if pattern.contains('/') {
                    let last_slash = pattern.rfind('/').unwrap();
                    format!("{}{}", &pattern[..=last_slash], e.name())
                } else {
                    e.name().to_string()
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
