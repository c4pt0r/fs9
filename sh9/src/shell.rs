//! Shell state and execution engine

use crate::error::{Sh9Error, Sh9Result};
use fs9_client::Fs9Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub struct BackgroundJob {
    pub id: usize,
    pub command: String,
    pub handle: JoinHandle<i32>,
}

pub struct Shell {
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub functions: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub last_exit_code: i32,
    pub client: Option<Arc<Fs9Client>>,
    pub server_url: String,
    pub jobs: Vec<BackgroundJob>,
    pub next_job_id: usize,
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
            jobs: Vec::new(),
            next_job_id: 1,
        }
    }

    /// Connect to the FS9 server
    pub async fn connect(&mut self) -> Sh9Result<()> {
        let client = Fs9Client::new(&self.server_url)
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
            jobs: Vec::new(),
            next_job_id: 1,
        }
    }
    
    pub fn add_job(&mut self, command: String, handle: JoinHandle<i32>) -> usize {
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.jobs.push(BackgroundJob { id, command, handle });
        id
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
