//! Evaluator for sh9script

use crate::ast::*;
use crate::error::{Sh9Error, Sh9Result};
use crate::help::{self, wants_help, get_help, format_help};
use crate::shell::Shell;
use fs9_client::OpenFlags;
use std::collections::HashMap;
use std::io::Write;
use std::pin::Pin;
use std::future::Future;

const STREAM_CHUNK_SIZE: usize = 64 * 1024;

fn format_mtime(mtime: u64) -> String {
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days_since_epoch = mtime / 86400;
    let time_of_day = mtime % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    
    let mut y = 1970i64;
    let mut remaining_days = days_since_epoch as i64;
    loop {
        let days_in_year = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
        if remaining_days < days_in_year { break; }
        remaining_days -= days_in_year;
        y += 1;
    }
    
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0usize;
    while m < 12 && remaining_days >= month_days[m] as i64 {
        remaining_days -= month_days[m] as i64;
        m += 1;
    }
    let d = remaining_days + 1;
    
    format!("{} {:>2} {:02}:{:02}", months[m], d, hours, minutes)
}

pub enum Output {
    Stdout,
    Buffer(Vec<u8>),
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
        }
    }

    pub fn writeln(&mut self, s: &str) -> std::io::Result<()> {
        self.write(s.as_bytes())?;
        self.write(b"\n")
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
                let _ = eprintln!("{}", msg);
            }
            Output::Buffer(buf) => {
                buf.extend_from_slice(msg.as_bytes());
                buf.push(b'\n');
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
    
    fn execute_statement_boxed<'a>(
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
            
            Statement::Pipeline(pipeline) => {
                if pipeline.background {
                    let cmd_str = pipeline.commands.iter()
                        .map(|c| {
                            let name = format!("{}", c.name);
                            let args = c.args.iter().map(|a| format!("{}", a)).collect::<Vec<_>>().join(" ");
                            if args.is_empty() { name } else { format!("{} {}", name, args) }
                        })
                        .collect::<Vec<_>>()
                        .join(" | ");
                    
                    let commands = pipeline.commands.clone();
                    let shell_copy = self.clone_for_subshell();
                    
                    let handle = tokio::spawn(async move {
                        let mut shell = shell_copy;
                        let mut ctx = ExecContext::default();
                        let pipeline = Pipeline { commands, background: false };
                        shell.execute_pipeline(&pipeline, &mut ctx).await.unwrap_or(1)
                    });
                    
                    self.add_job(cmd_str, handle);
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
        if pipeline.commands.is_empty() {
            return Ok(0);
        }
        
        if pipeline.commands.len() == 1 {
            return self.execute_command(&pipeline.commands[0], ctx).await;
        }
        
        let mut input: Option<Vec<u8>> = ctx.stdin.take();
        
        for (i, cmd) in pipeline.commands.iter().enumerate() {
            let is_last = i == pipeline.commands.len() - 1;
            
            ctx.stdin = input.take();
            
            if is_last {
                ctx.stdout = Output::Stdout;
                return self.execute_command(cmd, ctx).await;
            } else {
                ctx.stdout = Output::Buffer(Vec::new());
            }
            
            self.execute_command(cmd, ctx).await?;
            
            if let Output::Buffer(buf) = std::mem::replace(&mut ctx.stdout, Output::Buffer(Vec::new())) {
                input = Some(buf);
            }
        }
        
        Ok(0)
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
                                eprintln!("sh9: {}: {}", target, e);
                                return Ok(1);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        
        let has_stdout_redir = cmd.redirections.iter().any(|r| {
            matches!(r.kind, RedirectKind::StdoutWrite | RedirectKind::StdoutAppend | RedirectKind::BothWrite)
        });
        
        let has_stderr_redir = cmd.redirections.iter().any(|r| {
            matches!(r.kind, RedirectKind::StderrWrite | RedirectKind::StderrAppend | RedirectKind::BothWrite)
        });
        
        let saved_stdout = if has_stdout_redir {
            Some(std::mem::replace(&mut ctx.stdout, Output::Buffer(Vec::new())))
        } else {
            None
        };
        
        let saved_stderr = if has_stderr_redir {
            Some(std::mem::replace(&mut ctx.stderr, Output::Buffer(Vec::new())))
        } else {
            None
        };
        
        let result = self.execute_builtin(&name, &args, ctx).await;
        
        if has_stderr_redir {
            if let Output::Buffer(err_data) = std::mem::replace(&mut ctx.stderr, saved_stderr.unwrap()) {
                for redir in &cmd.redirections {
                    let target = self.expand_word(&redir.target, ctx).await?;
                    let path = self.resolve_path(&target);
                    
                    match &redir.kind {
                        RedirectKind::StderrWrite => {
                            if let Some(client) = &self.client {
                                client.write_file(&path, &err_data).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        RedirectKind::StderrAppend => {
                            if let Some(client) = &self.client {
                                let existing = client.read_file(&path).await.unwrap_or_default();
                                let mut combined = existing.to_vec();
                                combined.extend_from_slice(&err_data);
                                client.write_file(&path, &combined).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        RedirectKind::BothWrite => {
                            if let Some(client) = &self.client {
                                client.write_file(&path, &err_data).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        
        if has_stdout_redir {
            if let Output::Buffer(data) = std::mem::replace(&mut ctx.stdout, saved_stdout.unwrap()) {
                for redir in &cmd.redirections {
                    let target = self.expand_word(&redir.target, ctx).await?;
                    let path = self.resolve_path(&target);
                    
                    match &redir.kind {
                        RedirectKind::StdoutWrite => {
                            if let Some(client) = &self.client {
                                client.write_file(&path, &data).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        RedirectKind::StdoutAppend => {
                            if let Some(client) = &self.client {
                                let existing = client.read_file(&path).await.unwrap_or_default();
                                let mut combined = existing.to_vec();
                                combined.extend_from_slice(&data);
                                client.write_file(&path, &combined).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        RedirectKind::BothWrite => {
                            if let Some(client) = &self.client {
                                let existing = client.read_file(&path).await.unwrap_or_default();
                                let mut combined = existing.to_vec();
                                combined.extend_from_slice(&data);
                                client.write_file(&path, &combined).await
                                    .map_err(|e| Sh9Error::Runtime(format!("redirect: {}", e)))?;
                            }
                        }
                        _ => {}
                    }
                }
            }
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
        
        match name {
            "echo" => {
                let mut interpret_escapes = false;
                let mut no_newline = false;
                let mut text_args = Vec::new();
                
                for arg in args {
                    match arg.as_str() {
                        "-e" => interpret_escapes = true,
                        "-n" => no_newline = true,
                        "-en" | "-ne" => {
                            interpret_escapes = true;
                            no_newline = true;
                        }
                        _ => text_args.push(arg.as_str()),
                    }
                }
                
                let mut output = text_args.join(" ");
                
                if interpret_escapes {
                    output = interpret_escape_sequences(&output);
                }
                
                if no_newline {
                    ctx.stdout.write(output.as_bytes()).map_err(Sh9Error::Io)?;
                } else {
                    ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "pwd" => {
                ctx.stdout.writeln(&self.cwd).map_err(Sh9Error::Io)?;
                Ok(0)
            }
            
            "cd" => {
                let path = args.first().map(|s| s.as_str()).unwrap_or("/");
                let new_cwd = self.resolve_path(path);
                
                if let Some(client) = &self.client {
                    match client.stat(&new_cwd).await {
                        Ok(stat) if stat.is_dir() => {
                            self.cwd = new_cwd;
                            Ok(0)
                        }
                        Ok(_) => {
                            eprintln!("cd: {}: Not a directory", path);
                            Ok(1)
                        }
                        Err(_) => {
                            eprintln!("cd: {}: No such file or directory", path);
                            Ok(1)
                        }
                    }
                } else {
                    self.cwd = new_cwd;
                    Ok(0)
                }
            }
            
            "ls" => {
                let mut long_format = false;
                let mut path = ".";
                
                for arg in args {
                    if arg == "-l" {
                        long_format = true;
                    } else if arg == "-la" || arg == "-al" {
                        long_format = true;
                    } else if !arg.starts_with('-') {
                        path = arg;
                    }
                }
                
                let full_path = self.resolve_path(path);
                
                if let Some(client) = &self.client {
                    match client.readdir(&full_path).await {
                        Ok(entries) => {
                            for entry in entries {
                                if long_format {
                                    let type_char = if entry.is_dir() { 'd' } else { '-' };
                                    let mode = entry.mode;
                                    let mode_str = format!(
                                        "{}{}{}{}{}{}{}{}{}",
                                        if mode & 0o400 != 0 { 'r' } else { '-' },
                                        if mode & 0o200 != 0 { 'w' } else { '-' },
                                        if mode & 0o100 != 0 { 'x' } else { '-' },
                                        if mode & 0o040 != 0 { 'r' } else { '-' },
                                        if mode & 0o020 != 0 { 'w' } else { '-' },
                                        if mode & 0o010 != 0 { 'x' } else { '-' },
                                        if mode & 0o004 != 0 { 'r' } else { '-' },
                                        if mode & 0o002 != 0 { 'w' } else { '-' },
                                        if mode & 0o001 != 0 { 'x' } else { '-' },
                                    );
                                    let mtime_str = format_mtime(entry.mtime);
                                    let line = format!(
                                        "{}{} {} {} {:>6} {} {}",
                                        type_char,
                                        mode_str,
                                        entry.uid,
                                        entry.gid,
                                        entry.size,
                                        mtime_str,
                                        entry.name()
                                    );
                                    ctx.stdout.writeln(&line).map_err(Sh9Error::Io)?;
                                } else {
                                    ctx.stdout.writeln(entry.name()).map_err(Sh9Error::Io)?;
                                }
                            }
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("ls: {}: {}", path, e));
                            Ok(1)
                        }
                    }
                } else {
                    ctx.write_err("ls: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "tree" => {
                let mut max_depth: Option<usize> = None;
                let mut dirs_only = false;
                let mut show_hidden = false;
                let mut path = ".";
                
                let mut i = 0;
                while i < args.len() {
                    match args[i].as_str() {
                        "-L" if i + 1 < args.len() => {
                            max_depth = args[i + 1].parse().ok();
                            i += 2;
                        }
                        "-d" => {
                            dirs_only = true;
                            i += 1;
                        }
                        "-a" => {
                            show_hidden = true;
                            i += 1;
                        }
                        arg if !arg.starts_with('-') => {
                            path = arg;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                
                let full_path = self.resolve_path(path);
                ctx.stdout.writeln(&full_path).map_err(Sh9Error::Io)?;
                
                if let Some(client) = &self.client {
                    self.print_tree(client, &full_path, "", true, 0, max_depth, dirs_only, show_hidden, ctx).await?;
                } else {
                    ctx.write_err("tree: not connected to FS9 server");
                    return Ok(1);
                }
                Ok(0)
            }
            
            "cat" => {
                let stream_mode = args.iter().any(|a| a == "--stream" || a == "-s");
                let paths: Vec<&str> = args.iter()
                    .filter(|a| *a != "--stream" && *a != "-s" && *a != "-")
                    .map(|s| s.as_str())
                    .collect();
                let has_stdin = args.iter().any(|a| a == "-");
                
                if paths.is_empty() && !has_stdin {
                    if let Some(input) = ctx.stdin.take() {
                        ctx.stdout.write(&input).map_err(Sh9Error::Io)?;
                    }
                } else {
                    if has_stdin {
                        if let Some(input) = ctx.stdin.take() {
                            ctx.stdout.write(&input).map_err(Sh9Error::Io)?;
                        }
                    }
                    
                    for path in paths {
                        let full_path = self.resolve_path(path);
                        if let Some(client) = &self.client {
                            match client.open(&full_path, OpenFlags::read()).await {
                                Ok(handle) => {
                                    let mut offset = 0u64;
                                    if stream_mode {
                                        loop {
                                            match client.read(&handle, offset, STREAM_CHUNK_SIZE).await {
                                                Ok(data) if data.is_empty() => {
                                                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                                    continue;
                                                }
                                                Ok(data) => {
                                                    ctx.stdout.write(&data).map_err(Sh9Error::Io)?;
                                                    offset += data.len() as u64;
                                                }
                                                Err(e) => {
                                                    let _ = client.close(handle).await;
                                                    ctx.write_err(&format!("cat: {}: {}", path, e));
                                                    return Ok(1);
                                                }
                                            }
                                        }
                                    } else {
                                        loop {
                                            match client.read(&handle, offset, STREAM_CHUNK_SIZE).await {
                                                Ok(data) if data.is_empty() => break,
                                                Ok(data) => {
                                                    ctx.stdout.write(&data).map_err(Sh9Error::Io)?;
                                                    offset += data.len() as u64;
                                                }
                                                Err(e) => {
                                                    let _ = client.close(handle).await;
                                                    ctx.write_err(&format!("cat: {}: {}", path, e));
                                                    return Ok(1);
                                                }
                                            }
                                        }
                                        let _ = client.close(handle).await;
                                    }
                                }
                                Err(e) => {
                                    ctx.write_err(&format!("cat: {}: {}", path, e));
                                    return Ok(1);
                                }
                            }
                        } else {
                            ctx.write_err("cat: not connected to FS9 server");
                            return Ok(1);
                        }
                    }
                }
                Ok(0)
            }
            
            "mkdir" => {
                for path in args {
                    if path.starts_with('-') {
                        continue;
                    }
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        if let Err(e) = client.mkdir(&full_path).await {
                            eprintln!("mkdir: {}: {}", path, e);
                            return Ok(1);
                        }
                    } else {
                        eprintln!("mkdir: not connected to FS9 server");
                        return Ok(1);
                    }
                }
                Ok(0)
            }
            
            "touch" => {
                for path in args {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        if let Err(e) = client.write_file(&full_path, &[]).await {
                            eprintln!("touch: {}: {}", path, e);
                            return Ok(1);
                        }
                    } else {
                        eprintln!("touch: not connected to FS9 server");
                        return Ok(1);
                    }
                }
                Ok(0)
            }
            
            "truncate" => {
                let mut size: Option<usize> = None;
                let mut files: Vec<&String> = Vec::new();
                let mut i = 0;
                
                while i < args.len() {
                    if args[i] == "-s" && i + 1 < args.len() {
                        size = args[i + 1].parse().ok();
                        i += 2;
                    } else if !args[i].starts_with('-') {
                        files.push(&args[i]);
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                
                let target_size = size.unwrap_or(0);
                
                for path in files {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        let current = client.read_file(&full_path).await.unwrap_or_default();
                        let truncated: Vec<u8> = current.iter().take(target_size).copied().collect();
                        if let Err(e) = client.write_file(&full_path, &truncated).await {
                            eprintln!("truncate: {}: {}", path, e);
                            return Ok(1);
                        }
                    } else {
                        eprintln!("truncate: not connected to FS9 server");
                        return Ok(1);
                    }
                }
                Ok(0)
            }
            
            "rm" => {
                for path in args {
                    if path.starts_with('-') {
                        continue;
                    }
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        if let Err(e) = client.remove(&full_path).await {
                            eprintln!("rm: {}: {}", path, e);
                            return Ok(1);
                        }
                    } else {
                        eprintln!("rm: not connected to FS9 server");
                        return Ok(1);
                    }
                }
                Ok(0)
            }
            
            "mv" => {
                if args.len() != 2 {
                    eprintln!("mv: requires two arguments");
                    return Ok(1);
                }
                let src = self.resolve_path(&args[0]);
                let dst = self.resolve_path(&args[1]);
                
                if let Some(client) = &self.client {
                    if let Err(e) = client.rename(&src, &dst).await {
                        eprintln!("mv: {}", e);
                        return Ok(1);
                    }
                    Ok(0)
                } else {
                    eprintln!("mv: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "cp" => {
                if args.len() != 2 {
                    eprintln!("cp: requires two arguments");
                    return Ok(1);
                }
                let src = self.resolve_path(&args[0]);
                let dst = self.resolve_path(&args[1]);
                
                if let Some(client) = &self.client {
                    let src_handle = match client.open(&src, OpenFlags::read()).await {
                        Ok(h) => h,
                        Err(e) => {
                            eprintln!("cp: {}: {}", args[0], e);
                            return Ok(1);
                        }
                    };
                    let dst_handle = match client.open(&dst, OpenFlags::create_truncate()).await {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = client.close(src_handle).await;
                            eprintln!("cp: {}: {}", args[1], e);
                            return Ok(1);
                        }
                    };
                    
                    let mut offset = 0u64;
                    loop {
                        match client.read(&src_handle, offset, STREAM_CHUNK_SIZE).await {
                            Ok(data) if data.is_empty() => break,
                            Ok(data) => {
                                let len = data.len();
                                if let Err(e) = client.write(&dst_handle, offset, &data).await {
                                    let _ = client.close(src_handle).await;
                                    let _ = client.close(dst_handle).await;
                                    eprintln!("cp: write error: {}", e);
                                    return Ok(1);
                                }
                                offset += len as u64;
                            }
                            Err(e) => {
                                let _ = client.close(src_handle).await;
                                let _ = client.close(dst_handle).await;
                                eprintln!("cp: read error: {}", e);
                                return Ok(1);
                            }
                        }
                    }
                    let _ = client.close(src_handle).await;
                    let _ = client.close(dst_handle).await;
                    Ok(0)
                } else {
                    eprintln!("cp: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "stat" => {
                for path in args {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        match client.stat(&full_path).await {
                            Ok(stat) => {
                                ctx.stdout.writeln(&format!("file: {}", path)).map_err(Sh9Error::Io)?;
                                ctx.stdout.writeln(&format!("size: {}", stat.size)).map_err(Sh9Error::Io)?;
                                ctx.stdout.writeln(&format!("type: {}", if stat.is_dir() { "directory" } else { "file" })).map_err(Sh9Error::Io)?;
                            }
                            Err(e) => {
                                eprintln!("stat: {}: {}", path, e);
                                return Ok(1);
                            }
                        }
                    } else {
                        eprintln!("stat: not connected to FS9 server");
                        return Ok(1);
                    }
                }
                Ok(0)
            }
            
            "mount" => {
                if let Some(client) = &self.client {
                    match client.list_mounts().await {
                        Ok(mounts) => {
                            for m in mounts {
                                ctx.stdout.writeln(&format!("{} on {} type {}", m.provider_name, m.path, m.provider_name)).map_err(Sh9Error::Io)?;
                            }
                            Ok(0)
                        }
                        Err(e) => {
                            eprintln!("mount: {}", e);
                            Ok(1)
                        }
                    }
                } else {
                    eprintln!("mount: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "basename" => {
                if args.is_empty() {
                    eprintln!("basename: missing operand");
                    return Ok(1);
                }
                let path = &args[0];
                let suffix = args.get(1).map(|s| s.as_str());
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let result = if let Some(suf) = suffix {
                    name.strip_suffix(suf).unwrap_or(&name).to_string()
                } else {
                    name
                };
                ctx.stdout.writeln(&result).map_err(Sh9Error::Io)?;
                Ok(0)
            }
            
            "dirname" => {
                if args.is_empty() {
                    eprintln!("dirname: missing operand");
                    return Ok(1);
                }
                let path = &args[0];
                let parent = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string());
                let result = if parent.is_empty() { "." } else { &parent };
                ctx.stdout.writeln(result).map_err(Sh9Error::Io)?;
                Ok(0)
            }
            
            "date" => {
                use std::time::SystemTime;
                let now = SystemTime::now();
                let secs = now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
                
                let days_since_epoch = secs / 86400;
                let time_of_day = secs % 86400;
                let hours = time_of_day / 3600;
                let minutes = (time_of_day % 3600) / 60;
                let seconds = time_of_day % 60;
                
                let mut y = 1970i64;
                let mut remaining_days = days_since_epoch as i64;
                loop {
                    let days_in_year = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
                    if remaining_days < days_in_year { break; }
                    remaining_days -= days_in_year;
                    y += 1;
                }
                
                let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
                let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
                let mut m = 0usize;
                while m < 12 && remaining_days >= month_days[m] as i64 {
                    remaining_days -= month_days[m] as i64;
                    m += 1;
                }
                let d = remaining_days + 1;
                
                let format = args.first().map(|s| s.as_str());
                let output = if let Some(fmt) = format {
                    fmt.trim_matches('+')
                        .replace("%Y", &format!("{:04}", y))
                        .replace("%m", &format!("{:02}", m + 1))
                        .replace("%d", &format!("{:02}", d))
                        .replace("%H", &format!("{:02}", hours))
                        .replace("%M", &format!("{:02}", minutes))
                        .replace("%S", &format!("{:02}", seconds))
                } else {
                    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m + 1, d, hours, minutes, seconds)
                };
                ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
                Ok(0)
            }
            
            "sort" => {
                let reverse = args.iter().any(|a| a == "-r");
                let input = if let Some(data) = ctx.stdin.take() {
                    String::from_utf8_lossy(&data).to_string()
                } else if let Some(path) = args.iter().find(|a| !a.starts_with('-')) {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        match client.read_file(&full_path).await {
                            Ok(data) => String::from_utf8_lossy(&data).to_string(),
                            Err(e) => {
                                eprintln!("sort: {}: {}", path, e);
                                return Ok(1);
                            }
                        }
                    } else {
                        eprintln!("sort: not connected to FS9 server");
                        return Ok(1);
                    }
                } else {
                    String::new()
                };
                
                let mut lines: Vec<&str> = input.lines().collect();
                lines.sort();
                if reverse {
                    lines.reverse();
                }
                for line in lines {
                    ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "uniq" => {
                let input = if let Some(data) = ctx.stdin.take() {
                    String::from_utf8_lossy(&data).to_string()
                } else if let Some(path) = args.first() {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        match client.read_file(&full_path).await {
                            Ok(data) => String::from_utf8_lossy(&data).to_string(),
                            Err(e) => {
                                eprintln!("uniq: {}: {}", path, e);
                                return Ok(1);
                            }
                        }
                    } else {
                        eprintln!("uniq: not connected to FS9 server");
                        return Ok(1);
                    }
                } else {
                    String::new()
                };
                
                let mut prev: Option<&str> = None;
                for line in input.lines() {
                    if prev != Some(line) {
                        ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
                        prev = Some(line);
                    }
                }
                Ok(0)
            }
            
            "tr" => {
                fn expand_range(s: &str) -> Vec<char> {
                    let mut result = Vec::new();
                    let chars: Vec<char> = s.chars().collect();
                    let mut i = 0;
                    while i < chars.len() {
                        if i + 2 < chars.len() && chars[i + 1] == '-' {
                            let start = chars[i];
                            let end = chars[i + 2];
                            for c in start..=end {
                                result.push(c);
                            }
                            i += 3;
                        } else {
                            result.push(chars[i]);
                            i += 1;
                        }
                    }
                    result
                }
                
                if args.is_empty() {
                    eprintln!("tr: missing operand");
                    return Ok(1);
                }
                
                let delete_mode = args.first().map(|s| s == "-d").unwrap_or(false);
                let (set1, set2) = if delete_mode {
                    if args.len() < 2 {
                        eprintln!("tr: missing operand");
                        return Ok(1);
                    }
                    (&args[1], None)
                } else {
                    if args.len() < 2 {
                        eprintln!("tr: missing operand");
                        return Ok(1);
                    }
                    (&args[0], args.get(1))
                };
                
                let input = ctx.stdin.take().unwrap_or_default();
                let input_str = String::from_utf8_lossy(&input);
                
                let output: String = if delete_mode {
                    let del_chars = expand_range(set1);
                    input_str.chars().filter(|c| !del_chars.contains(c)).collect()
                } else if let Some(s2) = set2 {
                    let from = expand_range(set1);
                    let to = expand_range(s2);
                    input_str.chars().map(|c| {
                        if let Some(idx) = from.iter().position(|&x| x == c) {
                            to.get(idx).copied().unwrap_or(c)
                        } else {
                            c
                        }
                    }).collect()
                } else {
                    input_str.to_string()
                };
                
                ctx.stdout.write(output.as_bytes()).map_err(Sh9Error::Io)?;
                Ok(0)
            }
            
            "rev" => {
                let input = if let Some(data) = ctx.stdin.take() {
                    String::from_utf8_lossy(&data).to_string()
                } else if let Some(path) = args.first() {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        match client.read_file(&full_path).await {
                            Ok(data) => String::from_utf8_lossy(&data).to_string(),
                            Err(e) => {
                                eprintln!("rev: {}: {}", path, e);
                                return Ok(1);
                            }
                        }
                    } else {
                        eprintln!("rev: not connected to FS9 server");
                        return Ok(1);
                    }
                } else {
                    String::new()
                };
                
                for line in input.lines() {
                    let reversed: String = line.chars().rev().collect();
                    ctx.stdout.writeln(&reversed).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "cut" => {
                let mut delimiter = '\t';
                let mut fields: Option<Vec<usize>> = None;
                let mut chars: Option<(usize, Option<usize>)> = None;
                let mut file_path: Option<&str> = None;
                
                let mut i = 0;
                while i < args.len() {
                    match args[i].as_str() {
                        "-d" => {
                            if i + 1 < args.len() {
                                delimiter = args[i + 1].chars().next().unwrap_or('\t');
                                i += 2;
                            } else { i += 1; }
                        }
                        "-f" => {
                            if i + 1 < args.len() {
                                fields = Some(args[i + 1].split(',')
                                    .filter_map(|s| s.parse::<usize>().ok())
                                    .collect());
                                i += 2;
                            } else { i += 1; }
                        }
                        "-c" => {
                            if i + 1 < args.len() {
                                let range = &args[i + 1];
                                if let Some((start, end)) = range.split_once('-') {
                                    let s = start.parse::<usize>().unwrap_or(1);
                                    let e = if end.is_empty() { None } else { end.parse::<usize>().ok() };
                                    chars = Some((s, e));
                                }
                                i += 2;
                            } else { i += 1; }
                        }
                        s if !s.starts_with('-') => {
                            file_path = Some(s);
                            i += 1;
                        }
                        _ => { i += 1; }
                    }
                }
                
                let input = if let Some(data) = ctx.stdin.take() {
                    String::from_utf8_lossy(&data).to_string()
                } else if let Some(path) = file_path {
                    let full_path = self.resolve_path(path);
                    if let Some(client) = &self.client {
                        match client.read_file(&full_path).await {
                            Ok(data) => String::from_utf8_lossy(&data).to_string(),
                            Err(_) => String::new(),
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                
                for line in input.lines() {
                    let output = if let Some(ref f) = fields {
                        let parts: Vec<&str> = line.split(delimiter).collect();
                        f.iter()
                            .filter_map(|&i| parts.get(i.saturating_sub(1)))
                            .map(|s| *s)
                            .collect::<Vec<_>>()
                            .join(&delimiter.to_string())
                    } else if let Some((start, end)) = chars {
                        let chars_vec: Vec<char> = line.chars().collect();
                        let s = start.saturating_sub(1);
                        let e = end.unwrap_or(chars_vec.len());
                        chars_vec.get(s..e.min(chars_vec.len()))
                            .map(|c| c.iter().collect::<String>())
                            .unwrap_or_default()
                    } else {
                        line.to_string()
                    };
                    ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "tee" => {
                let append = args.iter().any(|a| a == "-a");
                let files: Vec<&str> = args.iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(|s| s.as_str())
                    .collect();
                
                let input = ctx.stdin.take().unwrap_or_default();
                
                ctx.stdout.write(&input).map_err(Sh9Error::Io)?;
                
                if let Some(client) = &self.client {
                    for file in files {
                        let full_path = self.resolve_path(file);
                        let flags = if append { OpenFlags::append() } else { OpenFlags::create_truncate() };
                        if let Ok(handle) = client.open(&full_path, flags).await {
                            let offset = if append { handle.size() } else { 0 };
                            let _ = client.write(&handle, offset, &input).await;
                            let _ = client.close(handle).await;
                        }
                    }
                }
                Ok(0)
            }
            
            "jq" => {
                let filter = args.first().map(|s| s.as_str()).unwrap_or(".");
                
                let input = if let Some(data) = ctx.stdin.take() {
                    String::from_utf8_lossy(&data).to_string()
                } else {
                    String::new()
                };
                
                let json: serde_json::Value = match serde_json::from_str(&input) {
                    Ok(v) => v,
                    Err(e) => {
                        ctx.write_err(&format!("jq: parse error: {}", e));
                        return Ok(1);
                    }
                };
                
                let result = self.jq_query(&json, filter);
                match result {
                    Ok(values) => {
                        for val in values {
                            let output = match &val {
                                serde_json::Value::String(s) => s.clone(),
                                _ => serde_json::to_string_pretty(&val).unwrap_or_default(),
                            };
                            ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
                        }
                        Ok(0)
                    }
                    Err(e) => {
                        ctx.write_err(&format!("jq: {}", e));
                        Ok(1)
                    }
                }
            }
            
            "env" => {
                for (name, value) in &self.env {
                    ctx.stdout.writeln(&format!("{}={}", name, value)).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "unset" => {
                for name in args {
                    self.env.remove(name);
                }
                Ok(0)
            }
            
            "source" | "." => {
                if args.is_empty() {
                    eprintln!("source: filename argument required");
                    return Ok(1);
                }
                let path = &args[0];
                let content = if let Some(client) = &self.client {
                    let full_path = self.resolve_path(path);
                    match client.read_file(&full_path).await {
                        Ok(data) => String::from_utf8_lossy(&data).to_string(),
                        Err(e) => {
                            eprintln!("source: {}: {}", path, e);
                            return Ok(1);
                        }
                    }
                } else {
                    match std::fs::read_to_string(path) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("source: {}: {}", path, e);
                            return Ok(1);
                        }
                    }
                };
                
                self.execute(&content).await
            }
            
            "true" => Ok(0),
            "false" => Ok(1),
            
            "help" => {
                if let Some(cmd_name) = args.first() {
                    if let Some(cmd_help) = get_help(cmd_name) {
                        ctx.stdout.write(format_help(cmd_help).as_bytes()).map_err(Sh9Error::Io)?;
                    } else {
                        ctx.write_err(&format!("help: no help for '{}'", cmd_name));
                        return Ok(1);
                    }
                } else {
                    ctx.stdout.write(help::format_help_list().as_bytes()).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "http" => {
                self.execute_http(args, ctx).await
            }
            
            "sleep" => {
                let secs = args.first()
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                tokio::time::sleep(tokio::time::Duration::from_secs_f64(secs)).await;
                Ok(0)
            }
            
            "jobs" => {
                for job in &self.jobs {
                    if !job.handle.is_finished() {
                        ctx.stdout.writeln(&format!("[{}] Running {}", job.id, job.command)).map_err(Sh9Error::Io)?;
                    }
                }
                Ok(0)
            }
            
            "wait" => {
                let jobs = std::mem::take(&mut self.jobs);
                for job in jobs {
                    let _ = job.handle.await;
                }
                Ok(0)
            }
            
            "exit" => {
                let code = args.first()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(0);
                Err(Sh9Error::Exit(code))
            }
            
            "export" => {
                for arg in args {
                    if let Some((name, value)) = arg.split_once('=') {
                        self.set_var(name, value);
                    }
                }
                Ok(0)
            }
            
            "set" => {
                for (name, value) in &self.env {
                    ctx.stdout.writeln(&format!("{}={}", name, value)).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "alias" => {
                if args.is_empty() {
                    let mut aliases: Vec<_> = self.aliases.iter().collect();
                    aliases.sort_by_key(|(k, _)| k.as_str());
                    for (name, value) in aliases {
                        ctx.stdout.writeln(&format!("alias {}='{}'", name, value)).map_err(Sh9Error::Io)?;
                    }
                } else {
                    let mut i = 0;
                    while i < args.len() {
                        if i + 2 < args.len() && args[i + 1] == "=" {
                            let name = &args[i];
                            let value = args[i + 2].trim_matches(|c| c == '\'' || c == '"');
                            self.aliases.insert(name.to_string(), value.to_string());
                            i += 3;
                        } else if let Some((name, value)) = args[i].split_once('=') {
                            let value = value.trim_matches(|c| c == '\'' || c == '"');
                            self.aliases.insert(name.to_string(), value.to_string());
                            i += 1;
                        } else if let Some(value) = self.aliases.get(&args[i]) {
                            ctx.stdout.writeln(&format!("alias {}='{}'", &args[i], value)).map_err(Sh9Error::Io)?;
                            i += 1;
                        } else {
                            i += 1;
                        }
                    }
                }
                Ok(0)
            }
            
            "unalias" => {
                for arg in args {
                    self.aliases.remove(arg);
                }
                Ok(0)
            }
            
            "local" => {
                // Handle args like ["x", "=", "5"] which should be "x=5"
                // or ["x"] which declares x with empty value
                let mut i = 0;
                while i < args.len() {
                    if i + 2 < args.len() && args[i + 1] == "=" {
                        // Pattern: name = value
                        let name = &args[i];
                        let value = &args[i + 2];
                        ctx.locals.insert(name.clone(), value.clone());
                        i += 3;
                    } else if let Some((name, value)) = args[i].split_once('=') {
                        // Pattern: name=value (single arg)
                        ctx.locals.insert(name.to_string(), value.to_string());
                        i += 1;
                    } else {
                        // Pattern: name (declare with empty value)
                        ctx.locals.insert(args[i].clone(), String::new());
                        i += 1;
                    }
                }
                Ok(0)
            }
            
            "[" | "test" => self.execute_test(args, ctx),
            
            "grep" => {
                let mut ignore_case = false;
                let mut invert_match = false;
                let mut show_line_numbers = false;
                let mut count_only = false;
                let mut only_matching = false;
                let mut word_match = false;
                let mut quiet_mode = false;
                let mut use_regex = false;
                let mut pattern = "";
                
                for arg in args {
                    match arg.as_str() {
                        "-i" => ignore_case = true,
                        "-v" => invert_match = true,
                        "-n" => show_line_numbers = true,
                        "-c" => count_only = true,
                        "-o" => only_matching = true,
                        "-w" => word_match = true,
                        "-q" => quiet_mode = true,
                        "-E" | "-e" => use_regex = true,
                        s if s.starts_with('-') && s.len() > 1 => {
                            // Handle combined flags like -in, -iv, -nv
                            for c in s[1..].chars() {
                                match c {
                                    'i' => ignore_case = true,
                                    'v' => invert_match = true,
                                    'n' => show_line_numbers = true,
                                    'c' => count_only = true,
                                    'o' => only_matching = true,
                                    'w' => word_match = true,
                                    'q' => quiet_mode = true,
                                    'E' | 'e' => use_regex = true,
                                    _ => {}
                                }
                            }
                        }
                        s if !s.starts_with('-') => {
                            pattern = s;
                            break;
                        }
                        _ => {}
                    }
                }
                
                let input = ctx.stdin.take().unwrap_or_default();
                let input_str = String::from_utf8_lossy(&input);
                let mut match_count = 0;
                let mut found_any = false;
                
                for (line_num, line) in input_str.lines().enumerate() {
                    let line_to_check = if ignore_case {
                        line.to_lowercase()
                    } else {
                        line.to_string()
                    };
                    let pattern_to_check = if ignore_case {
                        pattern.to_lowercase()
                    } else {
                        pattern.to_string()
                    };
                    
                    let matches = if use_regex && pattern_to_check.contains('|') {
                        pattern_to_check.split('|').any(|p| {
                            if word_match {
                                line_to_check.split(|c: char| !c.is_alphanumeric() && c != '_')
                                    .any(|word| word == p)
                            } else {
                                line_to_check.contains(p)
                            }
                        })
                    } else if word_match {
                        line_to_check.split(|c: char| !c.is_alphanumeric() && c != '_')
                            .any(|word| word == pattern_to_check)
                    } else {
                        line_to_check.contains(&pattern_to_check)
                    };
                    
                    let final_match = if invert_match { !matches } else { matches };
                    
                    if final_match {
                        found_any = true;
                        match_count += 1;
                        
                        if quiet_mode {
                            // In quiet mode, just return success on first match
                            return Ok(0);
                        }
                        
                        if !count_only {
                            let output = if only_matching && !invert_match {
                                if use_regex && pattern.contains('|') {
                                    let pat = if ignore_case { pattern.to_lowercase() } else { pattern.to_string() };
                                    pat.split('|')
                                        .find(|p| line_to_check.contains(*p))
                                        .unwrap_or("")
                                        .to_string()
                                } else {
                                    pattern.to_string()
                                }
                            } else {
                                line.to_string()
                            };
                            
                            if show_line_numbers {
                                ctx.stdout.writeln(&format!("{}:{}", line_num + 1, output)).map_err(Sh9Error::Io)?;
                            } else {
                                ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
                            }
                        }
                    }
                }
                
                if count_only && !quiet_mode {
                    ctx.stdout.writeln(&match_count.to_string()).map_err(Sh9Error::Io)?;
                }
                
                // Return 0 if matches found, 1 if not
                Ok(if found_any { 0 } else { 1 })
            }
            
            "wc" => {
                let input = ctx.stdin.take().unwrap_or_default();
                let input_str = String::from_utf8_lossy(&input);
                
                let count_lines = args.iter().any(|a| a == "-l");
                let count_words = args.iter().any(|a| a == "-w");
                let count_chars = args.iter().any(|a| a == "-c");
                
                if count_lines {
                    let lines = input_str.lines().count();
                    ctx.stdout.writeln(&lines.to_string()).map_err(Sh9Error::Io)?;
                } else if count_words {
                    let words = input_str.split_whitespace().count();
                    ctx.stdout.writeln(&words.to_string()).map_err(Sh9Error::Io)?;
                } else if count_chars {
                    let chars = input_str.len();
                    ctx.stdout.writeln(&chars.to_string()).map_err(Sh9Error::Io)?;
                } else {
                    let lines = input_str.lines().count();
                    let words = input_str.split_whitespace().count();
                    let chars = input_str.len();
                    ctx.stdout.writeln(&format!("{} {} {}", lines, words, chars)).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "head" => {
                let mut n: usize = 10;
                let mut i = 0;
                while i < args.len() {
                    let arg = &args[i];
                    if arg == "-n" && i + 1 < args.len() {
                        n = args[i + 1].parse().unwrap_or(10);
                        i += 2;
                    } else if arg.starts_with("-n") && arg.len() > 2 {
                        n = arg[2..].parse().unwrap_or(10);
                        i += 1;
                    } else if arg.starts_with('-') && arg[1..].chars().all(|c| c.is_ascii_digit()) {
                        n = arg[1..].parse().unwrap_or(10);
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                
                let input = ctx.stdin.take().unwrap_or_default();
                let input_str = String::from_utf8_lossy(&input);
                
                for (idx, line) in input_str.lines().enumerate() {
                    if idx >= n {
                        break;
                    }
                    ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "tail" => {
                let mut n: usize = 10;
                let mut i = 0;
                while i < args.len() {
                    let arg = &args[i];
                    if arg == "-n" && i + 1 < args.len() {
                        n = args[i + 1].parse().unwrap_or(10);
                        i += 2;
                    } else if arg.starts_with("-n") && arg.len() > 2 {
                        n = arg[2..].parse().unwrap_or(10);
                        i += 1;
                    } else if arg.starts_with('-') && arg[1..].chars().all(|c| c.is_ascii_digit()) {
                        n = arg[1..].parse().unwrap_or(10);
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                
                let input = ctx.stdin.take().unwrap_or_default();
                let input_str = String::from_utf8_lossy(&input);
                let lines: Vec<&str> = input_str.lines().collect();
                let start = lines.len().saturating_sub(n);
                
                for line in &lines[start..] {
                    ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
                }
                Ok(0)
            }
            
            "upload" => {
                let mut recursive = false;
                let mut paths: Vec<&str> = Vec::new();
                
                for arg in args {
                    match arg.as_str() {
                        "-r" | "-R" => recursive = true,
                        s if !s.starts_with('-') => paths.push(s),
                        _ => {}
                    }
                }
                
                if paths.len() < 2 {
                    ctx.write_err("upload: requires LOCAL_PATH and FS9_PATH");
                    return Ok(1);
                }
                
                let local_path = paths[0];
                let fs9_path = self.resolve_path(paths[1]);
                
                if let Some(client) = &self.client {
                    match self.upload_path(client, local_path, &fs9_path, recursive).await {
                        Ok(count) => {
                            ctx.stdout.writeln(&format!("Uploaded {} file(s)", count)).map_err(Sh9Error::Io)?;
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("upload: {}", e));
                            Ok(1)
                        }
                    }
                } else {
                    ctx.write_err("upload: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "download" => {
                let mut recursive = false;
                let mut paths: Vec<&str> = Vec::new();
                
                for arg in args {
                    match arg.as_str() {
                        "-r" | "-R" => recursive = true,
                        s if !s.starts_with('-') => paths.push(s),
                        _ => {}
                    }
                }
                
                if paths.len() < 2 {
                    ctx.write_err("download: requires FS9_PATH and LOCAL_PATH");
                    return Ok(1);
                }
                
                let fs9_path = self.resolve_path(paths[0]);
                let local_path = paths[1];
                
                if let Some(client) = &self.client {
                    match self.download_path(client, &fs9_path, local_path, recursive).await {
                        Ok(count) => {
                            ctx.stdout.writeln(&format!("Downloaded {} file(s)", count)).map_err(Sh9Error::Io)?;
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("download: {}", e));
                            Ok(1)
                        }
                    }
                } else {
                    ctx.write_err("download: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            "chroot" => {
                if args.is_empty() {
                    ctx.stdout.writeln(&format!("Current root: {}", self.get_var("FS9_CHROOT").unwrap_or("/"))).map_err(Sh9Error::Io)?;
                } else if args.first().map(|s| s.as_str()) == Some("--exit") {
                    self.env.remove("FS9_CHROOT");
                    ctx.stdout.writeln("Exited chroot").map_err(Sh9Error::Io)?;
                } else {
                    let new_root = self.resolve_path(&args[0]);
                    if let Some(client) = &self.client {
                        match client.stat(&new_root).await {
                            Ok(stat) if stat.is_dir() => {
                                self.set_var("FS9_CHROOT", &new_root);
                                self.cwd = "/".to_string();
                                ctx.stdout.writeln(&format!("Changed root to {}", new_root)).map_err(Sh9Error::Io)?;
                            }
                            Ok(_) => {
                                ctx.write_err(&format!("chroot: {}: Not a directory", args[0]));
                                return Ok(1);
                            }
                            Err(e) => {
                                ctx.write_err(&format!("chroot: {}: {}", args[0], e));
                                return Ok(1);
                            }
                        }
                    }
                }
                Ok(0)
            }
            
            "plugins" => {
                if let Some(client) = &self.client {
                    match client.list_mounts().await {
                        Ok(mounts) => {
                            ctx.stdout.writeln("Mounted plugins:").map_err(Sh9Error::Io)?;
                            for mount in mounts {
                                ctx.stdout.writeln(&format!("  {} -> {}", 
                                    mount.path, 
                                    mount.provider_name
                                )).map_err(Sh9Error::Io)?;
                            }
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("plugins: {}", e));
                            Ok(1)
                        }
                    }
                } else {
                    ctx.write_err("plugins: not connected to FS9 server");
                    Ok(1)
                }
            }
            
            _ => {
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
    }
    
    fn execute_test(&self, args: &[String], _ctx: &mut ExecContext) -> Sh9Result<i32> {
        let args: Vec<&str> = args.iter()
            .map(|s| s.as_str())
            .filter(|s| *s != "]")
            .collect();
        
        if args.is_empty() {
            return Ok(1);
        }
        
        let result = match args.as_slice() {
            [s1, "=", s2] | [s1, "==", s2] => s1 == s2,
            [s1, "!=", s2] => s1 != s2,
            [n1, "-eq", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a == b
            }
            [n1, "-ne", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a != b
            }
            [n1, "-lt", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a < b
            }
            [n1, "-le", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a <= b
            }
            [n1, "-gt", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a > b
            }
            [n1, "-ge", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a >= b
            }
            ["-n", s] => !s.is_empty(),
            ["-z", s] => s.is_empty(),
            [s] => !s.is_empty(),
            _ => false,
        };
        
        Ok(if result { 0 } else { 1 })
    }
    
    async fn execute_if(&mut self, if_stmt: &IfStatement, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let cond_result = self.execute_pipeline(&if_stmt.condition, ctx).await?;
        
        if cond_result == 0 {
            let mut result = 0;
            for stmt in &if_stmt.then_body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
            Ok(result)
        } else {
            for elif in &if_stmt.elif_clauses {
                let elif_result = self.execute_pipeline(&elif.condition, ctx).await?;
                if elif_result == 0 {
                    let mut result = 0;
                    for stmt in &elif.body {
                        result = self.execute_statement_boxed(stmt, ctx).await?;
                        if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                            return Ok(result);
                        }
                    }
                    return Ok(result);
                }
            }
            
            if let Some(else_body) = &if_stmt.else_body {
                let mut result = 0;
                for stmt in else_body {
                    result = self.execute_statement_boxed(stmt, ctx).await?;
                    if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                        return Ok(result);
                    }
                }
                Ok(result)
            } else {
                Ok(0)
            }
        }
    }
    
    async fn execute_for(&mut self, for_loop: &ForLoop, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut result = 0;
        
        let mut all_items = Vec::new();
        for item in &for_loop.items {
            let expanded = self.expand_word(item, ctx).await?;
            let glob_expanded = self.expand_glob(&expanded).await;
            for glob_item in glob_expanded {
                for part in glob_item.split_whitespace() {
                    all_items.push(part.to_string());
                }
            }
        }
        
        for value in all_items {
            self.set_var(&for_loop.variable, &value);
            
            for stmt in &for_loop.body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                
                if ctx.should_break {
                    ctx.should_break = false;
                    return Ok(result);
                }
                if ctx.should_continue {
                    ctx.should_continue = false;
                    break;
                }
                if ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
        }
        
        Ok(result)
    }
    
    async fn execute_while(&mut self, while_loop: &WhileLoop, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut result = 0;
        
        loop {
            let cond_result = self.execute_pipeline(&while_loop.condition, ctx).await?;
            if cond_result != 0 {
                break;
            }
            
            for stmt in &while_loop.body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                
                if ctx.should_break {
                    ctx.should_break = false;
                    return Ok(result);
                }
                if ctx.should_continue {
                    ctx.should_continue = false;
                    break;
                }
                if ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
        }
        
        Ok(result)
    }
    
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
    
    fn collect_balanced_parens(chars: &mut std::iter::Peekable<std::str::Chars>, initial_depth: usize) -> String {
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
    
    fn get_variable_value(&self, name: &str, ctx: &ExecContext) -> String {
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
    
    async fn execute_http(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let method = args.first().map(|s| s.to_uppercase()).unwrap_or_default();
        let url = args.get(1).cloned().unwrap_or_default();
        
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut body: Option<String> = None;
        
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "-H" | "--header" => {
                    if i + 1 < args.len() {
                        if let Some((k, v)) = args[i + 1].split_once(':') {
                            headers.push((k.trim().to_string(), v.trim().to_string()));
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "-d" | "--data" => {
                    if i + 1 < args.len() {
                        body = Some(args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        
        let client = reqwest::Client::new();
        let mut req = match method.as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            "PATCH" => client.patch(&url),
            _ => {
                eprintln!("http: unknown method: {}", method);
                return Ok(1);
            }
        };
        
        for (k, v) in headers {
            req = req.header(&k, &v);
        }
        
        if let Some(b) = body {
            req = req.body(b);
        }
        
        match req.send().await {
            Ok(resp) => {
                match resp.text().await {
                    Ok(text) => {
                        ctx.stdout.write(text.as_bytes()).map_err(Sh9Error::Io)?;
                        if !text.ends_with('\n') {
                            ctx.stdout.write(b"\n").map_err(Sh9Error::Io)?;
                        }
                        Ok(0)
                    }
                    Err(e) => {
                        eprintln!("http: {}", e);
                        Ok(1)
                    }
                }
            }
            Err(e) => {
                eprintln!("http: {}", e);
                Ok(1)
            }
        }
    }
    
    fn evaluate_arithmetic(&self, expr: &str, ctx: &ExecContext) -> Sh9Result<i64> {
        let expr = expr.trim();
        let expanded = self.expand_arithmetic_vars(expr, ctx)?;
        self.eval_arithmetic_expr(&expanded)
    }
    
    fn expand_arithmetic_vars(&self, expr: &str, ctx: &ExecContext) -> Sh9Result<String> {
        let mut result = String::new();
        let mut chars = expr.chars().peekable();
        
        while let Some(c) = chars.next() {
            if c == '$' {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = self.get_variable_value(&name, ctx);
                result.push_str(&value);
            } else if c.is_alphabetic() || c == '_' {
                let mut name = String::from(c);
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = self.get_variable_value(&name, ctx);
                if !value.is_empty() {
                    result.push_str(&value);
                } else {
                    result.push('0');
                }
            } else {
                result.push(c);
            }
        }
        
        Ok(result)
    }
    
    fn eval_arithmetic_expr(&self, expr: &str) -> Sh9Result<i64> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Ok(0);
        }
        self.parse_arithmetic_expr(&mut expr.chars().peekable())
    }
    
    fn parse_arithmetic_expr(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        self.parse_additive(chars)
    }
    
    fn parse_additive(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        let mut left = self.parse_multiplicative(chars)?;
        
        loop {
            self.skip_whitespace(chars);
            match chars.peek() {
                Some('+') => {
                    chars.next();
                    let right = self.parse_multiplicative(chars)?;
                    left += right;
                }
                Some('-') => {
                    chars.next();
                    let right = self.parse_multiplicative(chars)?;
                    left -= right;
                }
                _ => break,
            }
        }
        
        Ok(left)
    }
    
    fn parse_multiplicative(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        let mut left = self.parse_unary(chars)?;
        
        loop {
            self.skip_whitespace(chars);
            match chars.peek() {
                Some('*') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    left *= right;
                }
                Some('/') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    if right == 0 {
                        return Err(Sh9Error::Runtime("Division by zero".to_string()));
                    }
                    left /= right;
                }
                Some('%') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    if right == 0 {
                        return Err(Sh9Error::Runtime("Division by zero".to_string()));
                    }
                    left %= right;
                }
                _ => break,
            }
        }
        
        Ok(left)
    }
    
    fn parse_unary(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        self.skip_whitespace(chars);
        
        match chars.peek() {
            Some('-') => {
                chars.next();
                Ok(-self.parse_primary(chars)?)
            }
            Some('+') => {
                chars.next();
                self.parse_primary(chars)
            }
            _ => self.parse_primary(chars),
        }
    }
    
    fn parse_primary(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        self.skip_whitespace(chars);
        
        match chars.peek() {
            Some('(') => {
                chars.next();
                let value = self.parse_arithmetic_expr(chars)?;
                self.skip_whitespace(chars);
                if chars.next() != Some(')') {
                    return Err(Sh9Error::Runtime("Expected ')'".to_string()));
                }
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() => {
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                num.parse::<i64>()
                    .map_err(|_| Sh9Error::Runtime(format!("Invalid number: {}", num)))
            }
            Some(_) | None => Ok(0),
        }
    }
    
    fn skip_whitespace(&self, chars: &mut std::iter::Peekable<std::str::Chars>) {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    }
    
    pub fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else if path == "." {
            self.cwd.clone()
        } else if path == ".." {
            let parts: Vec<&str> = self.cwd.split('/').filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", parts[..parts.len() - 1].join("/"))
            }
        } else {
            if self.cwd == "/" {
                format!("/{}", path)
            } else {
                format!("{}/{}", self.cwd, path)
            }
        }
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
    
    async fn expand_glob(&self, pattern: &str) -> Vec<String> {
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
    
    fn print_tree<'a>(
        &'a self,
        client: &'a fs9_client::Fs9Client,
        path: &'a str,
        prefix: &'a str,
        _is_last: bool,
        depth: usize,
        max_depth: Option<usize>,
        dirs_only: bool,
        show_hidden: bool,
        ctx: &'a mut ExecContext,
    ) -> Pin<Box<dyn Future<Output = Sh9Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(max) = max_depth {
                if depth >= max {
                    return Ok(());
                }
            }
            
            let entries = match client.readdir(path).await {
                Ok(e) => e,
                Err(_) => return Ok(()),
            };
            
            let mut filtered: Vec<_> = entries
                .into_iter()
                .filter(|e| {
                    let name = e.name();
                    let is_hidden = name.starts_with('.');
                    if is_hidden && !show_hidden {
                        return false;
                    }
                    if dirs_only && !e.is_dir() {
                        return false;
                    }
                    true
                })
                .collect();
            
            filtered.sort_by(|a, b| a.name().cmp(b.name()));
            
            for (i, entry) in filtered.iter().enumerate() {
                let is_last_entry = i == filtered.len() - 1;
                let connector = if is_last_entry { "└── " } else { "├── " };
                let line = format!("{}{}{}", prefix, connector, entry.name());
                ctx.stdout.writeln(&line).map_err(Sh9Error::Io)?;
                
                if entry.is_dir() {
                    let new_prefix = format!("{}{}", prefix, if is_last_entry { "    " } else { "│   " });
                    let child_path = if path == "/" {
                        format!("/{}", entry.name())
                    } else {
                        format!("{}/{}", path, entry.name())
                    };
                    self.print_tree(client, &child_path, &new_prefix, is_last_entry, depth + 1, max_depth, dirs_only, show_hidden, ctx).await?;
                }
            }
            
            Ok(())
        })
    }
    
    fn jq_query(&self, json: &serde_json::Value, filter: &str) -> Result<Vec<serde_json::Value>, String> {
        if filter == "." {
            return Ok(vec![json.clone()]);
        }
        
        let mut current = vec![json.clone()];
        let parts: Vec<&str> = filter.split('.').filter(|s| !s.is_empty()).collect();
        
        for part in parts {
            let mut next = Vec::new();
            
            for val in current {
                if part == "[]" {
                    if let serde_json::Value::Array(arr) = val {
                        next.extend(arr);
                    }
                } else if part.ends_with("[]") {
                    let key = &part[..part.len() - 2];
                    if let serde_json::Value::Object(obj) = &val {
                        if let Some(serde_json::Value::Array(arr)) = obj.get(key) {
                            next.extend(arr.clone());
                        }
                    }
                } else if let serde_json::Value::Object(obj) = &val {
                    if let Some(v) = obj.get(part) {
                        next.push(v.clone());
                    }
                } else if let serde_json::Value::Array(arr) = &val {
                    if let Ok(idx) = part.parse::<usize>() {
                        if let Some(v) = arr.get(idx) {
                            next.push(v.clone());
                        }
                    }
                }
            }
            current = next;
        }
        
        Ok(current)
    }
    
    fn upload_path<'a>(
        &'a self,
        client: &'a fs9_client::Fs9Client,
        local_path: &'a str,
        fs9_path: &'a str,
        recursive: bool,
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
        Box::pin(async move {
            use std::fs;
            use std::path::Path;
            
            let local = Path::new(local_path);
            if !local.exists() {
                return Err(format!("'{}' does not exist", local_path));
            }
            
            let mut count = 0;
            
            if local.is_file() {
                let handle = client.open(fs9_path, OpenFlags::create_truncate()).await.map_err(|e| e.to_string())?;
                let mut file = fs::File::open(local).map_err(|e| e.to_string())?;
                let mut offset = 0u64;
                let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
                loop {
                    use std::io::Read;
                    let n = file.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 { break; }
                    client.write(&handle, offset, &buf[..n]).await.map_err(|e| e.to_string())?;
                    offset += n as u64;
                }
                let _ = client.close(handle).await;
                count = 1;
            } else if local.is_dir() {
                if !recursive {
                    return Err(format!("'{}' is a directory (use -r)", local_path));
                }
                
                let _ = client.mkdir(fs9_path).await;
                
                let entries: Vec<_> = fs::read_dir(local)
                    .map_err(|e| e.to_string())?
                    .filter_map(|e| e.ok())
                    .collect();
                
                for entry in entries {
                    let child_local = entry.path();
                    let child_name = entry.file_name().to_string_lossy().to_string();
                    let child_fs9 = format!("{}/{}", fs9_path.trim_end_matches('/'), child_name);
                    let child_local_str = child_local.to_string_lossy().to_string();
                    
                    count += self.upload_path(client, &child_local_str, &child_fs9, recursive).await?;
                }
            }
            
            Ok(count)
        })
    }
    
    fn download_path<'a>(
        &'a self,
        client: &'a fs9_client::Fs9Client,
        fs9_path: &'a str,
        local_path: &'a str,
        recursive: bool,
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
        Box::pin(async move {
            use std::fs;
            use std::path::Path;
            
            let stat = client.stat(fs9_path).await.map_err(|e| e.to_string())?;
            let mut count = 0;
            
            if stat.is_dir() {
                if !recursive {
                    return Err(format!("'{}' is a directory (use -r)", fs9_path));
                }
                
                let local = Path::new(local_path);
                if !local.exists() {
                    fs::create_dir_all(local).map_err(|e| e.to_string())?;
                }
                
                let entries = client.readdir(fs9_path).await.map_err(|e| e.to_string())?;
                for entry in entries {
                    let child_fs9 = format!("{}/{}", fs9_path.trim_end_matches('/'), entry.name());
                    let child_local = format!("{}/{}", local_path.trim_end_matches('/'), entry.name());
                    
                    count += self.download_path(client, &child_fs9, &child_local, recursive).await?;
                }
            } else {
                let local = Path::new(local_path);
                if let Some(parent) = local.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                }
                
                let handle = client.open(fs9_path, OpenFlags::read()).await.map_err(|e| e.to_string())?;
                let mut file = fs::File::create(local_path).map_err(|e| e.to_string())?;
                let mut offset = 0u64;
                loop {
                    let data = client.read(&handle, offset, STREAM_CHUNK_SIZE).await.map_err(|e| e.to_string())?;
                    if data.is_empty() { break; }
                    use std::io::Write;
                    file.write_all(&data).map_err(|e| e.to_string())?;
                    offset += data.len() as u64;
                }
                let _ = client.close(handle).await;
                count = 1;
            }
            
            Ok(count)
        })
    }
}

fn interpret_escape_sequences(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('0') => result.push('\0'),
                Some('a') => result.push('\x07'),
                Some('b') => result.push('\x08'),
                Some('f') => result.push('\x0C'),
                Some('v') => result.push('\x0B'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn contains_glob_chars(s: &str) -> bool {
    s.chars().any(|c| c == '*' || c == '?' || c == '[')
}

/// Glob pattern matching: *, ?, [abc], [a-z], [!abc]
fn match_glob_pattern(pattern: &str, name: &str) -> bool {
    let mut pattern_chars = pattern.chars().peekable();
    let mut name_chars = name.chars().peekable();
    
    while let Some(p) = pattern_chars.next() {
        match p {
            '*' => {
                if pattern_chars.peek().is_none() {
                    return true;
                }
                let remaining_pattern: String = pattern_chars.collect();
                let mut remaining_name: String = name_chars.collect();
                
                loop {
                    if match_glob_pattern(&remaining_pattern, &remaining_name) {
                        return true;
                    }
                    if remaining_name.is_empty() {
                        return false;
                    }
                    remaining_name = remaining_name[1..].to_string();
                }
            }
            '?' => {
                if name_chars.next().is_none() {
                    return false;
                }
            }
            '[' => {
                let mut chars_in_class = Vec::new();
                let mut negated = false;
                let mut first = true;
                
                while let Some(c) = pattern_chars.next() {
                    if c == ']' && !first {
                        break;
                    }
                    if (c == '!' || c == '^') && first {
                        negated = true;
                        first = false;
                        continue;
                    }
                    first = false;
                    
                    if pattern_chars.peek() == Some(&'-') {
                        pattern_chars.next();
                        if let Some(end) = pattern_chars.next() {
                            if end != ']' {
                                for ch in c..=end {
                                    chars_in_class.push(ch);
                                }
                                continue;
                            } else {
                                chars_in_class.push(c);
                                chars_in_class.push('-');
                                break;
                            }
                        }
                    }
                    chars_in_class.push(c);
                }
                
                if let Some(n) = name_chars.next() {
                    let matched = chars_in_class.contains(&n);
                    if negated == matched {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            c => {
                if name_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }
    
    name_chars.next().is_none()
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
