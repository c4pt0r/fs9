use crate::error::{Sh9Error, Sh9Result};
use crate::help;
use crate::shell::Shell;
use super::ExecContext;

impl Shell {
    pub(crate) async fn try_execute_shell_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Option<Sh9Result<i32>> {
        match name {
            "true" | "false" | "exit" | "export" | "set" | "unset" | "env"
            | "local" | "alias" | "unalias" | "source" | "." | "sleep"
            | "jobs" | "fg" | "bg" | "kill" | "wait" | "help" | "http"
            | "upload" | "download" | "[" | "test" => {
                Some(self.dispatch_shell_builtin(name, args, ctx).await)
            }
            _ => None,
        }
    }

    async fn dispatch_shell_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        match name {
            "true" => Ok(0),
            "false" => Ok(1),
            "exit" => self.cmd_exit(args),
            "export" => self.cmd_export(args),
            "set" => self.cmd_set(ctx),
            "unset" => self.cmd_unset(args),
            "env" => self.cmd_env(ctx),
            "local" => self.cmd_local(args, ctx),
            "alias" => self.cmd_alias(args, ctx),
            "unalias" => self.cmd_unalias(args),
            "source" | "." => self.cmd_source(args, ctx).await,
            "sleep" => self.cmd_sleep(args).await,
            "jobs" => self.cmd_jobs(ctx),
            "fg" => self.cmd_fg(args, ctx).await,
            "bg" => self.cmd_bg(ctx),
            "kill" => self.cmd_kill(args, ctx),
            "wait" => self.cmd_wait().await,
            "help" => self.cmd_help(args, ctx),
            "http" => self.execute_http(args, ctx).await,
            "upload" => self.cmd_upload(args, ctx).await,
            "download" => self.cmd_download(args, ctx).await,
            "[" | "test" => self.execute_test(args, ctx).await,
            _ => unreachable!(),
        }
    }

    fn cmd_exit(&mut self, args: &[String]) -> Sh9Result<i32> {
        let code = args.first()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);
        Err(Sh9Error::Exit(code))
    }

    fn cmd_export(&mut self, args: &[String]) -> Sh9Result<i32> {
        let mut i = 0;
        while i < args.len() {
            // Handle "NAME = VALUE" (3 tokens from lexer splitting FOO=bar)
            if i + 2 < args.len() && args[i + 1] == "=" {
                self.set_var(&args[i], &args[i + 2]);
                i += 3;
            } else if let Some((name, value)) = args[i].split_once('=') {
                self.set_var(name, value);
                i += 1;
            } else {
                // export without value — no-op
                i += 1;
            }
        }
        Ok(0)
    }

    fn cmd_set(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        for (name, value) in &self.env {
            ctx.stdout.writeln(&format!("{}={}", name, value)).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    fn cmd_unset(&mut self, args: &[String]) -> Sh9Result<i32> {
        for name in args {
            self.env.remove(name);
        }
        Ok(0)
    }

    fn cmd_env(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        for (name, value) in &self.env {
            ctx.stdout.writeln(&format!("{}={}", name, value)).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    fn cmd_local(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut i = 0;
        while i < args.len() {
            if i + 2 < args.len() && args[i + 1] == "=" {
                let name = &args[i];
                let value = &args[i + 2];
                ctx.locals.insert(name.clone(), value.clone());
                i += 3;
            } else if let Some((name, value)) = args[i].split_once('=') {
                ctx.locals.insert(name.to_string(), value.to_string());
                i += 1;
            } else {
                ctx.locals.insert(args[i].clone(), String::new());
                i += 1;
            }
        }
        Ok(0)
    }

    fn cmd_alias(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_unalias(&mut self, args: &[String]) -> Sh9Result<i32> {
        for arg in args {
            self.aliases.remove(arg);
        }
        Ok(0)
    }

    async fn cmd_source(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            ctx.write_err("source: filename argument required");
            return Ok(1);
        }
        let path = &args[0];
        let content = if let Some(client) = &self.client {
            let full_path = self.resolve_path(path);
            match client.read_file(&full_path).await {
                Ok(data) => String::from_utf8_lossy(&data).to_string(),
                Err(e) => {
                    ctx.write_err(&format!("source: {}: {}", path, e));
                    return Ok(1);
                }
            }
        } else {
            match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    ctx.write_err(&format!("source: {}: {}", path, e));
                    return Ok(1);
                }
            }
        };
        
        self.execute(&content).await
    }

    async fn cmd_sleep(&mut self, args: &[String]) -> Sh9Result<i32> {
        let secs = args.first()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        tokio::time::sleep(tokio::time::Duration::from_secs_f64(secs)).await;
        Ok(0)
    }

    fn cmd_jobs(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        self.update_job_statuses();

        for job in &self.jobs {
            let status_str = match job.status {
                crate::shell::JobStatus::Running => "Running",
                crate::shell::JobStatus::Done(code) => {
                    if code == 0 {
                        "Done"
                    } else {
                        "Exit"
                    }
                }
            };
            ctx.stdout.writeln(&format!("[{}] {} {}", job.id, status_str, job.command)).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    async fn cmd_fg(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let job_id = if args.is_empty() {
            self.get_current_job().map(|j| j.id)
        } else {
            args[0].parse::<usize>().ok()
        };

        if let Some(id) = job_id {
            let job_index = self.jobs.iter().position(|j| j.id == id);
            if let Some(idx) = job_index {
                let job = self.jobs.remove(idx);
                ctx.stdout.writeln(&job.command).map_err(Sh9Error::Io)?;

                match job.handle.await {
                    Ok(code) => {
                        self.last_exit_code = code;
                        Ok(code)
                    }
                    Err(_) => {
                        ctx.write_err("fg: job terminated abnormally");
                        Ok(1)
                    }
                }
            } else {
                ctx.write_err(&format!("fg: job [{}] not found", id));
                Ok(1)
            }
        } else {
            ctx.write_err("fg: no current job");
            Ok(1)
        }
    }

    fn cmd_bg(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        ctx.write_err("bg: not implemented (sh9 doesn't support job control)");
        Ok(1)
    }

    fn cmd_kill(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            ctx.write_err("kill: usage: kill <job_id>");
            return Ok(1);
        }

        for arg in args {
            let job_id = if let Some(id_str) = arg.strip_prefix('%') {
                id_str.parse::<usize>().ok()
            } else {
                arg.parse::<usize>().ok()
            };

            if let Some(id) = job_id {
                let job_index = self.jobs.iter().position(|j| j.id == id);
                if let Some(idx) = job_index {
                    let job = self.jobs.remove(idx);
                    job.handle.abort();
                    ctx.stdout.writeln(&format!("[{}] Terminated {}", id, job.command)).map_err(Sh9Error::Io)?;
                } else {
                    ctx.write_err(&format!("kill: job [{}] not found", id));
                }
            } else {
                ctx.write_err(&format!("kill: invalid job id: {}", arg));
            }
        }
        Ok(0)
    }

    async fn cmd_wait(&mut self) -> Sh9Result<i32> {
        let jobs = std::mem::take(&mut self.jobs);
        for job in jobs {
            let _ = job.handle.await;
        }
        Ok(0)
    }

    fn cmd_help(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if let Some(cmd_name) = args.first() {
            if let Some(cmd_help) = crate::help::get_help(cmd_name) {
                ctx.stdout.write(crate::help::format_help(cmd_help).as_bytes()).map_err(Sh9Error::Io)?;
            } else {
                ctx.write_err(&format!("help: no help for '{}'", cmd_name));
                return Ok(1);
            }
        } else {
            ctx.stdout.write(help::format_help_list().as_bytes()).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    pub(crate) async fn execute_http(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
                ctx.write_err(&format!("http: unknown method: {}", method));
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
                        ctx.write_err(&format!("http: {}", e));
                        Ok(1)
                    }
                }
            }
            Err(e) => {
                ctx.write_err(&format!("http: {}", e));
                Ok(1)
            }
        }
    }

    async fn cmd_upload(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    async fn cmd_download(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
}
