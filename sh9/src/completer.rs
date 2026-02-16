use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use sh9::eval::namespace::Namespace;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::RwLock;
use fs9_client::Fs9Client;

pub struct Sh9Helper {
    pub client: Option<Arc<Fs9Client>>,
    pub cwd: Arc<RwLock<String>>,
    pub namespace: Arc<RwLock<Namespace>>,
    pub runtime: tokio::runtime::Handle,
}

impl Sh9Helper {
    pub fn new(
        client: Option<Arc<Fs9Client>>,
        cwd: Arc<RwLock<String>>,
        namespace: Arc<RwLock<Namespace>>,
    ) -> Self {
        Self {
            client,
            cwd,
            namespace,
            runtime: tokio::runtime::Handle::current(),
        }
    }
}

const BUILTINS: &[&str] = &[
    "alias", "basename", "bind", "break", "cat", "cd", "chroot", "continue", "cp", "cut",
    "date", "dirname", "download", "echo", "env", "exit", "export", "false", "grep",
    "head", "help", "http", "jobs", "jq", "local", "ls", "mkdir", "mount",
    "mv", "ns", "plugin", "pwd", "return", "rev", "rm", "set", "sleep", "sort", "source",
    "stat", "tail", "tee", "test", "touch", "tr", "tree", "true", "truncate",
    "unalias", "uniq", "unmount", "unset", "upload", "wait", "wc",
];

impl Completer for Sh9Helper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let line_to_cursor = &line[..pos];
        
        let (start, word) = find_word_start(line_to_cursor);
        
        if word.is_empty() {
            return Ok((pos, vec![]));
        }
        
        let is_first_word = !line_to_cursor[..start].contains(|c: char| !c.is_whitespace());
        
        let mut completions = Vec::new();
        
        if is_first_word {
            for &builtin in BUILTINS {
                if builtin.starts_with(word) {
                    completions.push(Pair {
                        display: builtin.to_string(),
                        replacement: builtin.to_string(),
                    });
                }
            }
        }
        
        if word.starts_with('/') || word.starts_with('.') || word.contains('/') || !is_first_word {
            let cwd = self.cwd.read().unwrap().clone();

            let (dir_path, partial_name) = if let Some(last_slash) = word.rfind('/') {
                let dir = &word[..=last_slash];
                let name = &word[last_slash + 1..];
                (resolve_path(&cwd, dir.trim_end_matches('/')), name)
            } else {
                (cwd.clone(), word)
            };

            let partial_owned = partial_name.to_string();

            let local_completions = {
                let ns = self.namespace.read().unwrap();
                let resolutions = ns.resolve(&dir_path);
                if resolutions.is_empty() {
                    None
                } else {
                    let mut names = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for (source, rel) in &resolutions {
                        let local_dir = source.join(rel.trim_start_matches('/'));
                        if let Ok(entries) = std::fs::read_dir(&local_dir) {
                            for entry in entries.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name.starts_with(&*partial_owned) && seen.insert(name.clone()) {
                                    let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
                                    names.push(if is_dir { format!("{name}/") } else { name });
                                }
                            }
                        }
                    }
                    Some(names)
                }
            };

            let path_completions = if let Some(local) = local_completions {
                local
            } else if let Some(client) = &self.client {
                let client = client.clone();
                let dir_path_owned = dir_path.clone();
                tokio::task::block_in_place(|| {
                    self.runtime.block_on(async {
                        complete_path(&client, &dir_path_owned, &partial_owned).await
                    })
                })
            } else {
                vec![]
            };

            for name in path_completions {
                let replacement = if word.contains('/') {
                    let last_slash = word.rfind('/').unwrap();
                    format!("{}{}", &word[..=last_slash], name)
                } else {
                    name.clone()
                };
                completions.push(Pair {
                    display: name,
                    replacement,
                });
            }
        }
        
        Ok((start, completions))
    }
}

async fn complete_path(client: &Fs9Client, dir: &str, partial: &str) -> Vec<String> {
    match client.readdir(dir).await {
        Ok(entries) => {
            entries
                .into_iter()
                .filter(|e| e.name().starts_with(partial))
                .map(|e| {
                    if e.is_dir() {
                        format!("{}/", e.name())
                    } else {
                        e.name().to_string()
                    }
                })
                .collect()
        }
        Err(_) => vec![],
    }
}

fn find_word_start(line: &str) -> (usize, &str) {
    let mut start = line.len();
    for (i, c) in line.char_indices().rev() {
        if c.is_whitespace() || c == ';' || c == '|' || c == '&' || c == '>' || c == '<' {
            break;
        }
        start = i;
    }
    (start, &line[start..])
}

fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else if path == "." {
        cwd.to_string()
    } else if path == ".." {
        let parts: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts[..parts.len() - 1].join("/"))
        }
    } else if path.is_empty() {
        cwd.to_string()
    } else if cwd == "/" {
        format!("/{}", path)
    } else {
        format!("{}/{}", cwd, path)
    }
}

impl Hinter for Sh9Helper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for Sh9Helper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Borrowed(hint)
    }
}

impl Validator for Sh9Helper {}

impl Helper for Sh9Helper {}
