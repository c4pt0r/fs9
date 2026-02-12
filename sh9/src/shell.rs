//! Shell state and execution engine

use crate::error::{Sh9Error, Sh9Result};
use fs9_client::Fs9Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Function signature for custom built-in commands registered via [`Shell::register_builtin`] or [`ShellBuilder::builtin`].
pub type BuiltinFn = dyn Fn(&[String], &mut Shell) -> Sh9Result<i32> + Send + Sync;

/// Captured output from [`Shell::execute_capture`].
#[derive(Debug, Clone)]
pub struct CapturedOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobStatus {
    Running,
    Done(i32),  // exit code
}

pub struct BackgroundJob {
    pub id: usize,
    pub command: String,
    pub handle: JoinHandle<i32>,
    pub status: JobStatus,
}

pub struct Shell {
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub functions: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub last_exit_code: i32,
    pub client: Option<Arc<Fs9Client>>,
    pub server_url: String,
    pub token: Option<String>,
    pub jobs: Vec<BackgroundJob>,
    pub next_job_id: usize,
    pub custom_builtins: HashMap<String, Arc<Box<BuiltinFn>>>,
}

impl Shell {
    pub fn new(server_url: &str) -> Self {
        Self {
            cwd: "/".to_string(),
            env: HashMap::new(),
            functions: HashMap::new(),
            aliases: HashMap::new(),
            last_exit_code: 0,
            client: None,
            server_url: server_url.to_string(),
            token: None,
            jobs: Vec::new(),
            next_job_id: 1,
            custom_builtins: HashMap::new(),
        }
    }

    /// Set the authentication token
    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }

    /// Connect to the FS9 server
    pub async fn connect(&mut self) -> Sh9Result<()> {
        let mut builder = Fs9Client::builder(&self.server_url);
        if let Some(ref token) = self.token {
            builder = builder.token(token);
        }
        let client = builder.build()
            .map_err(|e| Sh9Error::Fs9(e.to_string()))?;
        self.client = Some(Arc::new(client));
        Ok(())
    }

    /// Execute a command string
    pub async fn execute(&mut self, input: &str) -> Sh9Result<i32> {
        let script = crate::parser::parse(input)
            .map_err(|errs| {
                let msg = errs.into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                Sh9Error::Parse(msg)
            })?;
        
        self.execute_script(&script).await
    }

    /// Get the FS9 client, returning error if not connected
    pub fn client(&self) -> Sh9Result<&Fs9Client> {
        self.client
            .as_ref()
            .map(|c| c.as_ref())
            .ok_or_else(|| Sh9Error::Runtime("Not connected to FS9 server".to_string()))
    }

    /// Set an environment variable
    pub fn set_var(&mut self, name: &str, value: &str) {
        self.env.insert(name.to_string(), value.to_string());
    }

    /// Get an environment variable
    pub fn get_var(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(|s| s.as_str())
    }

    /// Define a function
    pub fn define_function(&mut self, name: &str, body: &str) {
        self.functions.insert(name.to_string(), body.to_string());
    }

    /// Get a function definition
    pub fn get_function(&self, name: &str) -> Option<&str> {
        self.functions.get(name).map(|s| s.as_str())
    }

    pub fn clone_for_subshell(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            functions: self.functions.clone(),
            aliases: self.aliases.clone(),
            last_exit_code: self.last_exit_code,
            client: self.client.clone(),
            server_url: self.server_url.clone(),
            token: self.token.clone(),
            jobs: Vec::new(),
            next_job_id: 1,
            custom_builtins: self.custom_builtins.clone(),
        }
    }
    
    pub fn add_job(&mut self, command: String, handle: JoinHandle<i32>) -> usize {
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.jobs.push(BackgroundJob {
            id,
            command,
            handle,
            status: JobStatus::Running,
        });
        id
    }

    /// Update job statuses (check which jobs are done)
    pub fn update_job_statuses(&mut self) {
        for job in &mut self.jobs {
            if job.status == JobStatus::Running && job.handle.is_finished() {
                // Note: We can't get the exit code without consuming the handle
                job.status = JobStatus::Done(0);
            }
        }
    }

    /// Remove completed jobs
    pub fn cleanup_jobs(&mut self) {
        self.jobs.retain(|job| job.status == JobStatus::Running);
    }

    /// Get a job by ID
    pub fn get_job(&self, id: usize) -> Option<&BackgroundJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Get the most recent job
    pub fn get_current_job(&self) -> Option<&BackgroundJob> {
        self.jobs.last()
    }

    pub fn register_builtin<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(&[String], &mut Shell) -> Sh9Result<i32> + Send + Sync + 'static,
    {
        self.custom_builtins
            .insert(name.to_string(), Arc::new(Box::new(handler)));
    }

    pub async fn execute_capture(&mut self, input: &str) -> Sh9Result<CapturedOutput> {
        let script = crate::parser::parse(input).map_err(|errs| {
            let msg = errs
                .into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Sh9Error::Parse(msg)
        })?;

        let mut ctx = crate::eval::ExecContext {
            stdout: crate::eval::Output::Buffer(Vec::new()),
            stderr: crate::eval::Output::Buffer(Vec::new()),
            ..crate::eval::ExecContext::default()
        };

        let mut last_exit = 0;
        for stmt in &script.statements {
            last_exit = self.execute_statement_boxed(stmt, &mut ctx).await?;
            self.last_exit_code = last_exit;
            if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                break;
            }
        }

        let stdout = match ctx.stdout {
            crate::eval::Output::Buffer(buf) => buf,
            _ => Vec::new(),
        };
        let stderr = match ctx.stderr {
            crate::eval::Output::Buffer(buf) => buf,
            _ => Vec::new(),
        };

        Ok(CapturedOutput {
            exit_code: last_exit,
            stdout,
            stderr,
        })
    }
}

pub struct ShellBuilder {
    server_url: String,
    token: Option<String>,
    env: HashMap<String, String>,
    builtins: HashMap<String, Arc<Box<BuiltinFn>>>,
}

impl ShellBuilder {
    pub fn new(server_url: &str) -> Self {
        Self {
            server_url: server_url.to_string(),
            token: None,
            env: HashMap::new(),
            builtins: HashMap::new(),
        }
    }

    pub fn token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    pub fn builtin<F>(mut self, name: &str, handler: F) -> Self
    where
        F: Fn(&[String], &mut Shell) -> Sh9Result<i32> + Send + Sync + 'static,
    {
        self.builtins
            .insert(name.to_string(), Arc::new(Box::new(handler)));
        self
    }

    pub fn build(self) -> Shell {
        Shell {
            cwd: "/".to_string(),
            env: self.env,
            functions: HashMap::new(),
            aliases: HashMap::new(),
            last_exit_code: 0,
            client: None,
            server_url: self.server_url,
            token: self.token,
            jobs: Vec::new(),
            next_job_id: 1,
            custom_builtins: self.builtins,
        }
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new("http://localhost:8080")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_creation() {
        let shell = Shell::new("http://localhost:8080");
        assert_eq!(shell.cwd, "/");
        assert_eq!(shell.last_exit_code, 0);
    }

    #[test]
    fn test_variable_operations() {
        let mut shell = Shell::default();
        shell.set_var("FOO", "bar");
        assert_eq!(shell.get_var("FOO"), Some("bar"));
        assert_eq!(shell.get_var("NONEXISTENT"), None);
    }

    #[test]
    fn test_function_operations() {
        let mut shell = Shell::default();
        shell.define_function("greet", "echo Hello");
        assert_eq!(shell.get_function("greet"), Some("echo Hello"));
        assert_eq!(shell.get_function("nonexistent"), None);
    }
}
