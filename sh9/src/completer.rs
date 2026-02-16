use chumsky::Parser;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use sh9::eval::namespace::Namespace;
use sh9::lexer::{lexer, Token, QuoteType};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::RwLock;
use fs9_client::Fs9Client;

pub struct Sh9Helper {
    pub client: Option<Arc<Fs9Client>>,
    pub cwd: Arc<RwLock<String>>,
    pub namespace: Arc<RwLock<Namespace>>,
    pub runtime: tokio::runtime::Handle,
    pub env: Arc<RwLock<HashMap<String, String>>>,
    pub aliases: Arc<RwLock<HashMap<String, String>>>,
    pub functions: Arc<RwLock<HashSet<String>>>,
}

impl Sh9Helper {
    pub fn new(
        client: Option<Arc<Fs9Client>>,
        cwd: Arc<RwLock<String>>,
        namespace: Arc<RwLock<Namespace>>,
        env: Arc<RwLock<HashMap<String, String>>>,
        aliases: Arc<RwLock<HashMap<String, String>>>,
        functions: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            client,
            cwd,
            namespace,
            runtime: tokio::runtime::Handle::current(),
            env,
            aliases,
            functions,
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
        
        // Variable completion: $VAR
        if let Some(var_prefix) = word.strip_prefix('$') {
            let env = self.env.read().unwrap();
            let mut completions = Vec::new();
            
            for var_name in env.keys() {
                if var_name.starts_with(var_prefix) {
                    completions.push(Pair {
                        display: format!("{}  (env)", var_name),
                        replacement: format!("${}", var_name),
                    });
                }
            }
            
            completions.sort_by(|a, b| a.replacement.cmp(&b.replacement));
            return Ok((start, completions));
        }
        
        let is_first_word = !line_to_cursor[..start].contains(|c: char| !c.is_whitespace());
        
        let mut completions = Vec::new();
        
        if is_first_word {
            // Builtin commands
            for &builtin in BUILTINS {
                if builtin.starts_with(word) {
                    completions.push(Pair {
                        display: format!("{}  (builtin)", builtin),
                        replacement: builtin.to_string(),
                    });
                }
            }
            
            // Aliases
            let aliases = self.aliases.read().unwrap();
            for alias_name in aliases.keys() {
                if alias_name.starts_with(word) {
                    completions.push(Pair {
                        display: format!("{}  (alias)", alias_name),
                        replacement: alias_name.clone(),
                    });
                }
            }
            
            // Functions
            let functions = self.functions.read().unwrap();
            for func_name in functions.iter() {
                if func_name.starts_with(word) {
                    completions.push(Pair {
                        display: format!("{}  (function)", func_name),
                        replacement: func_name.clone(),
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
                
                let display = if name.ends_with('/') {
                    format!("{}  (dir)", name)
                } else {
                    name.clone()
                };
                
                completions.push(Pair {
                    display,
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

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        // Only show hints when cursor is at end of line
        if pos < line.len() || line.is_empty() {
            return None;
        }
        
        // Search history backwards for entries starting with current line
        let history = ctx.history();
        for i in (0..history.len()).rev() {
            if let Ok(Some(entry)) = history.get(i, rustyline::history::SearchDirection::Reverse) {
                if entry.entry.starts_with(line) && entry.entry.len() > line.len() {
                    return Some(entry.entry[line.len()..].to_string());
                }
            }
        }
        None
    }
}

impl Highlighter for Sh9Helper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        // Empty input - no highlighting
        if line.is_empty() {
            return Cow::Borrowed(line);
        }

        // Try to tokenize the input
        let tokens = match lexer().parse(line) {
            Ok(tokens) => tokens,
            Err(_) => return Cow::Borrowed(line), // Partial input - no highlighting
        };

        // If no tokens, return unchanged
        if tokens.is_empty() {
            return Cow::Borrowed(line);
        }

        // Build highlighted string by walking through the original input
        let mut result = String::with_capacity(line.len() + 100); // Extra space for ANSI codes
        let mut pos = 0;
        let mut is_first_word = true;

        for token in &tokens {
            // Get the token's text representation
            let token_text = token.to_string();
            
            // Skip newlines in highlighting (they're not visible)
            if matches!(token, Token::Newline) {
                continue;
            }

            // Find the token in the remaining input
            if let Some(token_start) = line[pos..].find(&token_text) {
                // Add any whitespace/characters before the token
                result.push_str(&line[pos..pos + token_start]);
                pos += token_start;

                // Determine color based on token type
                let (color_start, color_end) = match token {
                    // First word: check if it's a builtin command
                    Token::Word(w) if is_first_word => {
                        is_first_word = false;
                        if BUILTINS.contains(&w.as_str()) {
                            ("\x1b[32m", "\x1b[0m") // Green for known commands
                        } else {
                            ("\x1b[31m", "\x1b[0m") // Red for unknown commands
                        }
                    }
                    // Strings
                    Token::SingleQuoted(_) => ("\x1b[33m", "\x1b[0m"), // Yellow
                    Token::CompoundWord(segments) => {
                        // Check if it contains quoted parts
                        let has_quotes = segments.iter().any(|(qt, _)| {
                            !matches!(qt, QuoteType::Bare)
                        });
                        if has_quotes {
                            ("\x1b[33m", "\x1b[0m") // Yellow for quoted parts
                        } else {
                            ("", "") // No color for bare compound words
                        }
                    }
                    // Variables
                    Token::Dollar
                    | Token::DollarBrace
                    | Token::DollarParen
                    | Token::DollarDoubleParen => ("\x1b[36m", "\x1b[0m"), // Cyan
                    // Operators
                    Token::Pipe
                    | Token::AndAnd
                    | Token::OrOr
                    | Token::Semicolon
                    | Token::Ampersand => ("\x1b[1m", "\x1b[0m"), // Bold
                    // Redirections
                    Token::RedirectOut
                    | Token::RedirectAppend
                    | Token::RedirectIn
                    | Token::RedirectErr
                    | Token::RedirectErrAppend
                    | Token::RedirectBoth
                    | Token::HereDoc
                    | Token::HereString => ("\x1b[1m", "\x1b[0m"), // Bold
                    // Everything else (keywords, brackets, etc.)
                    _ => {
                        // After first word, subsequent words are not commands
                        if matches!(token, Token::Word(_)) {
                            is_first_word = false;
                        }
                        ("", "")
                    }
                };

                // Add colored token
                result.push_str(color_start);
                result.push_str(&token_text);
                result.push_str(color_end);
                pos += token_text.len();
            } else {
                // Token not found in expected position - fallback to no highlighting
                return Cow::Borrowed(line);
            }
        }

        // Add any remaining characters
        if pos < line.len() {
            result.push_str(&line[pos..]);
        }

        Cow::Owned(result)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Enable highlighting on every keystroke
        true
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}

impl Validator for Sh9Helper {}

impl Helper for Sh9Helper {}
