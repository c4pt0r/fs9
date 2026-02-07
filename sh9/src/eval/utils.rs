use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;
use fs9_client::OpenFlags;
use std::pin::Pin;
use std::future::Future;
use super::{ExecContext, STREAM_CHUNK_SIZE};

pub(crate) fn format_mtime(mtime: u64) -> String {
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

pub(crate) fn interpret_escape_sequences(s: &str) -> String {
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

pub(crate) fn contains_glob_chars(s: &str) -> bool {
    s.chars().any(|c| c == '*' || c == '?' || c == '[')
}

pub(crate) fn match_glob_pattern(pattern: &str, name: &str) -> bool {
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

impl Shell {
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

    pub(crate) fn print_tree<'a>(
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

    pub(crate) fn jq_query(&self, json: &serde_json::Value, filter: &str) -> Result<Vec<serde_json::Value>, String> {
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

    pub(crate) fn upload_path<'a>(
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

    pub(crate) fn download_path<'a>(
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
