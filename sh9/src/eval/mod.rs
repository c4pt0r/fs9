//! Evaluator for sh9script

use crate::ast::*;
use crate::error::{Sh9Error, Sh9Result};
use crate::help::{wants_help, get_help, format_help};
use crate::shell::Shell;
use fs9_client::Fs9Client;
use std::collections::HashMap;
use std::io::Write;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;

mod arithmetic;
mod builtins_fs;
mod builtins_shell;
mod builtins_text;
mod control_flow;
mod expansion;
mod utils;
#[allow(dead_code)]
mod local_fs;
mod namespace;
#[allow(dead_code)]
mod router;

pub(crate) const STREAM_CHUNK_SIZE: usize = 64 * 1024;

pub enum Output {
    Stdout,
    Buffer(Vec<u8>),
    File {
        client: Arc<Fs9Client>,
        path: String,
        buffer: Vec<u8>,
        mode: FileWriteMode,
    },
}

#[derive(Clone, Copy)]
pub enum FileWriteMode {
    Write,   // Overwrite
    Append,  // Append
}

impl Output {
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Output::Stdout => {
                std::io::stdout().write_all(data)?;
                std::io::stdout().flush()
            }
            Output::Buffer(buf) => {
                buf.extend_from_slice(data);
                Ok(())
            }
            Output::File { buffer, .. } => {
                // Just buffer the data, flush will write it
                buffer.extend_from_slice(data);
                Ok(())
            }
        }
    }

    pub fn writeln(&mut self, s: &str) -> std::io::Result<()> {
        self.write(s.as_bytes())?;
        self.write(b"\n")
    }

    pub async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Output::Stdout => std::io::stdout().flush(),
            Output::Buffer(_) => Ok(()),
            Output::File { client, path, buffer, mode } => {
                if !buffer.is_empty() {
                    let to_write = std::mem::take(buffer);
                    if let Err(e) = match mode {
                        FileWriteMode::Write => {
                            let result = client.write_file(&path, &to_write).await;
                            // After first write, switch to append mode
                            *mode = FileWriteMode::Append;
                            result
                        }
                        FileWriteMode::Append => {
                            let existing: bytes::Bytes = client.read_file(&path).await.unwrap_or_default();
                            let mut combined = existing.to_vec();
                            combined.extend_from_slice(&to_write);
                            client.write_file(&path, &combined).await
                        }
                    } {
                        let msg = format!("Error flushing to {}: {}\n", path, e);
                        let _ = std::io::stderr().write_all(msg.as_bytes());
                    }
                }
                Ok(())
            }
        }
    }
}

pub struct ExecContext {
    pub locals: HashMap<String, String>,
    pub positional: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub stdout: Output,
    pub stderr: Output,
    pub should_break: bool,
    pub should_continue: bool,
    pub return_value: Option<i32>,
}

impl Default for ExecContext {
    fn default() -> Self {
        Self {
            locals: HashMap::new(),
            positional: Vec::new(),
            stdin: None,
            stdout: Output::Stdout,
            stderr: Output::Stdout,
            should_break: false,
            should_continue: false,
            return_value: None,
        }
    }
}

impl ExecContext {
    pub fn write_err(&mut self, msg: &str) {
        match &mut self.stderr {
            Output::Stdout => {
                let msg_with_newline = format!("{}\n", msg);
                let _ = std::io::stderr().write_all(msg_with_newline.as_bytes());
            }
            Output::Buffer(buf) => {
                buf.extend_from_slice(msg.as_bytes());
                buf.push(b'\n');
            }
            Output::File { buffer, .. } => {
                buffer.extend_from_slice(msg.as_bytes());
                buffer.push(b'\n');
            }
        }
    }
}

impl Shell {
    pub async fn execute_script(&mut self, script: &Script) -> Sh9Result<i32> {
        let mut ctx = ExecContext::default();
        let mut last_exit = 0;
        
        for stmt in &script.statements {
            last_exit = self.execute_statement_boxed(stmt, &mut ctx).await?;
            self.last_exit_code = last_exit;
            
            if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                break;
            }
        }
        
        Ok(last_exit)
    }
    
    pub(crate) fn execute_statement_boxed<'a>(
        &'a mut self,
        stmt: &'a Statement,
        ctx: &'a mut ExecContext,
    ) -> Pin<Box<dyn Future<Output = Sh9Result<i32>> + Send + 'a>> {
        Box::pin(self.execute_statement(stmt, ctx))
    }
    
    async fn execute_statement(&mut self, stmt: &Statement, ctx: &mut ExecContext) -> Sh9Result<i32> {
        match stmt {
            Statement::Empty => Ok(0),
            
            Statement::Assignment(assign) => {
                let value = self.expand_word(&assign.value, ctx).await?;
                if ctx.locals.contains_key(&assign.name) {
                    ctx.locals.insert(assign.name.clone(), value);
                } else {
                    self.set_var(&assign.name, &value);
                }
                Ok(0)
            }
            
            Statement::CommandList { first, rest } => {
                let mut exit_code = self.execute_pipeline(first, ctx).await?;
                self.last_exit_code = exit_code;
                for (op, pipeline) in rest {
                    match op {
                        ListOp::And => {
                            if exit_code == 0 {
                                exit_code = self.execute_pipeline(pipeline, ctx).await?;
                                self.last_exit_code = exit_code;
                            }
                        }
                        ListOp::Or => {
                            if exit_code != 0 {
                                exit_code = self.execute_pipeline(pipeline, ctx).await?;
                                self.last_exit_code = exit_code;
                            }
                        }
                    }
                }
                Ok(exit_code)
            }

            Statement::Pipeline(pipeline) => {
                if pipeline.background {
                    // Format command string
                    let cmd_str = pipeline.elements.iter()
                        .map(|elem| match elem {
                            PipelineElement::Simple(c) => {
                                let name = format!("{}", c.name);
                                let args = c.args.iter().map(|a| format!("{}", a)).collect::<Vec<_>>().join(" ");
                                let mut cmd = if args.is_empty() { name } else { format!("{} {}", name, args) };
                                for redir in &c.redirections {
                                    let target = format!("{}", redir.target);
                                    let redir_str = match redir.kind {
                                        RedirectKind::StdoutWrite => format!(" > {}", target),
                                        RedirectKind::StdoutAppend => format!(" >> {}", target),
                                        RedirectKind::StdinRead => format!(" < {}", target),
                                        RedirectKind::StderrWrite => format!(" 2> {}", target),
                                        RedirectKind::StderrAppend => format!(" 2>> {}", target),
                                        RedirectKind::BothWrite => format!(" &> {}", target),
                                    };
                                    cmd.push_str(&redir_str);
                                }
                                cmd
                            }
                            PipelineElement::Compound(stmt) => format!("{:?}", stmt),
                        })
                        .collect::<Vec<_>>()
                        .join(" | ");

                    let elements = pipeline.elements.clone();
                    let shell_copy = self.clone_for_subshell();

                    let handle = tokio::spawn(async move {
                        let mut shell = shell_copy;
                        let mut ctx = ExecContext::default();
                        let pipeline = Pipeline { elements, background: false };
                        match shell.execute_pipeline(&pipeline, &mut ctx).await {
                            Ok(code) => code,
                            Err(e) => {
                                let msg = format!("Background job error: {}\n", e);
                                let _ = std::io::stderr().write_all(msg.as_bytes());
                                1
                            }
                        }
                    });

                    let job_id = self.add_job(cmd_str, handle);
                    ctx.write_err(&format!("[{}] Started", job_id));
                    Ok(0)
                } else {
                    self.execute_pipeline(pipeline, ctx).await
                }
            }
            
            Statement::If(if_stmt) => {
                self.execute_if(if_stmt, ctx).await
            }
            
            Statement::For(for_loop) => {
                self.execute_for(for_loop, ctx).await
            }
            
            Statement::While(while_loop) => {
                self.execute_while(while_loop, ctx).await
            }
            
            Statement::FunctionDef(func) => {
                let body_str = serde_json::to_string(&func.body)
                    .map_err(|e| Sh9Error::Runtime(format!("Failed to serialize function: {}", e)))?;
                self.define_function(&func.name, &body_str);
                Ok(0)
            }
            
            Statement::Break => {
                ctx.should_break = true;
                Ok(0)
            }
            
            Statement::Continue => {
                ctx.should_continue = true;
                Ok(0)
            }
            
            Statement::Return(value) => {
                let code = if let Some(word) = value {
                    let expanded = self.expand_word(word, ctx).await?;
                    expanded.parse::<i32>().unwrap_or(0)
                } else {
                    0
                };
                ctx.return_value = Some(code);
                Ok(code)
            }
        }
    }
    
    async fn execute_pipeline(&mut self, pipeline: &Pipeline, ctx: &mut ExecContext) -> Sh9Result<i32> {
        if pipeline.elements.is_empty() {
            return Ok(0);
        }

        if pipeline.elements.len() == 1 {
            return self.execute_pipeline_element(&pipeline.elements[0], ctx).await;
        }

        // Save the original stdout so the last element writes to the correct destination
        let mut original_stdout = Some(std::mem::replace(&mut ctx.stdout, Output::Buffer(Vec::new())));
        let mut input: Option<Vec<u8>> = ctx.stdin.take();

        for (i, elem) in pipeline.elements.iter().enumerate() {
            let is_last = i == pipeline.elements.len() - 1;

            ctx.stdin = input.take();

            if is_last {
                ctx.stdout = original_stdout.take().unwrap_or(Output::Stdout);
                return self.execute_pipeline_element(elem, ctx).await;
            } else {
                ctx.stdout = Output::Buffer(Vec::new());
            }

            self.execute_pipeline_element(elem, ctx).await?;

            if let Output::Buffer(buf) = std::mem::replace(&mut ctx.stdout, Output::Buffer(Vec::new())) {
                input = Some(buf);
            }
        }

        Ok(0)
    }

    async fn execute_pipeline_element(&mut self, elem: &PipelineElement, ctx: &mut ExecContext) -> Sh9Result<i32> {
        match elem {
            PipelineElement::Simple(cmd) => self.execute_command(cmd, ctx).await,
            PipelineElement::Compound(stmt) => self.execute_statement_boxed(stmt, ctx).await,
        }
    }
    
    async fn execute_command(&mut self, cmd: &Command, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let raw_name = self.expand_word(&cmd.name, ctx).await?;
        
        let (name, mut args) = if let Some(alias_value) = self.aliases.get(&raw_name) {
            let parts: Vec<&str> = alias_value.split_whitespace().collect();
            if parts.is_empty() {
                (raw_name, Vec::new())
            } else {
                let alias_name = parts[0].to_string();
                let alias_args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
                (alias_name, alias_args)
            }
        } else {
            (raw_name, Vec::new())
        };
        
        for arg in &cmd.args {
            let expanded = self.expand_word(arg, ctx).await?;
            let glob_expanded = self.expand_glob(&expanded).await;
            args.extend(glob_expanded);
        }
        
        for redir in &cmd.redirections {
            let target = self.expand_word(&redir.target, ctx).await?;
            match &redir.kind {
                RedirectKind::StdinRead => {
                    let path = self.resolve_path(&target);
                    if let Some(client) = &self.client {
                        match client.read_file(&path).await {
                            Ok(data) => ctx.stdin = Some(data.to_vec()),
                            Err(e) => {
                                ctx.write_err(&format!("sh9: {}: {}", target, e));
                                return Ok(1);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        
        // Set up output redirections
        let mut stdout_redir_path: Option<(String, FileWriteMode)> = None;
        let mut stderr_redir_path: Option<(String, FileWriteMode)> = None;

        for redir in &cmd.redirections {
            let target = self.expand_word(&redir.target, ctx).await?;
            let path = self.resolve_path(&target);

            match &redir.kind {
                RedirectKind::StdoutWrite => {
                    stdout_redir_path = Some((path, FileWriteMode::Write));
                }
                RedirectKind::StdoutAppend => {
                    stdout_redir_path = Some((path, FileWriteMode::Append));
                }
                RedirectKind::BothWrite => {
                    stdout_redir_path = Some((path.clone(), FileWriteMode::Write));
                    stderr_redir_path = Some((path, FileWriteMode::Write));
                }
                RedirectKind::StderrWrite => {
                    stderr_redir_path = Some((path, FileWriteMode::Write));
                }
                RedirectKind::StderrAppend => {
                    stderr_redir_path = Some((path, FileWriteMode::Append));
                }
                _ => {}
            }
        }

        let saved_stdout = if let Some((path, mode)) = stdout_redir_path {
            if let Some(client) = &self.client {
                Some(std::mem::replace(&mut ctx.stdout, Output::File {
                    client: client.clone(),
                    path,
                    buffer: Vec::new(),
                    mode,
                }))
            } else {
                None
            }
        } else {
            None
        };

        let saved_stderr = if let Some((path, mode)) = stderr_redir_path {
            if let Some(client) = &self.client {
                Some(std::mem::replace(&mut ctx.stderr, Output::File {
                    client: client.clone(),
                    path,
                    buffer: Vec::new(),
                    mode,
                }))
            } else {
                None
            }
        } else {
            None
        };
        
        let result = self.execute_builtin(&name, &args, ctx).await;

        // Flush and restore stderr
        if saved_stderr.is_some() {
            ctx.stderr.flush().await.map_err(Sh9Error::Io)?;
            ctx.stderr = saved_stderr.unwrap();
        }

        // Flush and restore stdout
        if saved_stdout.is_some() {
            ctx.stdout.flush().await.map_err(Sh9Error::Io)?;
            ctx.stdout = saved_stdout.unwrap();
        }
        
        if let Ok(code) = &result {
            self.last_exit_code = *code;
        }
        
        result
    }
    
    fn show_help_if_requested(name: &str, args: &[String], ctx: &mut ExecContext) -> Option<Sh9Result<i32>> {
        if wants_help(args) {
            if let Some(cmd_help) = get_help(name) {
                let _ = ctx.stdout.write(format_help(cmd_help).as_bytes());
                return Some(Ok(0));
            }
        }
        None
    }
    
    async fn execute_builtin(&mut self, name: &str, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if let Some(result) = Self::show_help_if_requested(name, args, ctx) {
            return result;
        }
        
        // Try text builtins
        if let Some(result) = self.try_execute_text_builtin(name, args, ctx).await {
            return result;
        }
        
        // Try fs builtins
        if let Some(result) = self.try_execute_fs_builtin(name, args, ctx).await {
            return result;
        }
        
        // Try shell builtins
        if let Some(result) = self.try_execute_shell_builtin(name, args, ctx).await {
            return result;
        }

        // Try custom builtins (registered via register_builtin)
        if let Some(handler) = self.custom_builtins.get(name).cloned() {
            return handler(args, self);
        }
        
        // Try as user-defined function
        if let Some(body_str) = self.get_function(name).map(|s| s.to_string()) {
            let body: Vec<Statement> = serde_json::from_str(&body_str)
                .map_err(|e| Sh9Error::Runtime(format!("Failed to parse function: {}", e)))?;
            
            let inherited_stdout = std::mem::replace(&mut ctx.stdout, Output::Stdout);
            let inherited_stderr = std::mem::replace(&mut ctx.stderr, Output::Stdout);
            let mut func_ctx = ExecContext {
                locals: HashMap::new(),
                positional: args.to_vec(),
                stdin: ctx.stdin.take(),
                stdout: inherited_stdout,
                stderr: inherited_stderr,
                should_break: false,
                should_continue: false,
                return_value: None,
            };
            
            let mut result = 0;
            for stmt in &body {
                result = self.execute_statement_boxed(stmt, &mut func_ctx).await?;
                if func_ctx.return_value.is_some() {
                    ctx.stdout = func_ctx.stdout;
                    ctx.stderr = func_ctx.stderr;
                    return Ok(func_ctx.return_value.unwrap());
                }
            }
            
            ctx.stdout = func_ctx.stdout;
            ctx.stderr = func_ctx.stderr;
            Ok(result)
        } else {
            Err(Sh9Error::CommandNotFound(name.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_echo() {
        let mut shell = Shell::new("http://localhost:8080");
        let script = crate::parser::parse("echo hello").unwrap();
        let result = shell.execute_script(&script).await.unwrap();
        assert_eq!(result, 0);
    }
    
    #[tokio::test]
    async fn test_variable_assignment() {
        let mut shell = Shell::new("http://localhost:8080");
        let script = crate::parser::parse("x=5").unwrap();
        shell.execute_script(&script).await.unwrap();
        assert_eq!(shell.get_var("x"), Some("5"));
    }
    
    #[tokio::test]
    async fn test_true_false() {
        let mut shell = Shell::new("http://localhost:8080");
        
        let script = crate::parser::parse("true").unwrap();
        let result = shell.execute_script(&script).await.unwrap();
        assert_eq!(result, 0);
        
        let script = crate::parser::parse("false").unwrap();
        let result = shell.execute_script(&script).await.unwrap();
        assert_eq!(result, 1);
    }
}
