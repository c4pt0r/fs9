use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;
use fs9_client::OpenFlags;
use super::{ExecContext, STREAM_CHUNK_SIZE};
use super::utils::interpret_escape_sequences;

impl Shell {
    pub(crate) async fn try_execute_text_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Option<Sh9Result<i32>> {
        match name {
            "echo" | "printf" | "grep" | "wc" | "head" | "tail" | "sort" | "uniq"
            | "tr" | "rev" | "cut" | "tee" | "jq" | "date" | "seq" | "cat" | "read" => {
                Some(self.dispatch_text_builtin(name, args, ctx).await)
            }
            _ => None,
        }
    }

    async fn dispatch_text_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        match name {
            "echo" => self.cmd_echo(args, ctx),
            "printf" => self.cmd_printf(args, ctx),
            "cat" => self.cmd_cat(args, ctx).await,
            "grep" => self.cmd_grep(args, ctx),
            "wc" => self.cmd_wc(args, ctx),
            "head" => self.cmd_head(args, ctx),
            "tail" => self.cmd_tail(args, ctx).await,
            "sort" => self.cmd_sort(args, ctx).await,
            "uniq" => self.cmd_uniq(args, ctx).await,
            "tr" => self.cmd_tr(args, ctx),
            "rev" => self.cmd_rev(args, ctx).await,
            "cut" => self.cmd_cut(args, ctx).await,
            "tee" => self.cmd_tee(args, ctx).await,
            "jq" => self.cmd_jq(args, ctx),
            "date" => self.cmd_date(args, ctx),
            "seq" => self.cmd_seq(args, ctx),
            "read" => self.cmd_read(args, ctx),
            _ => unreachable!(),
        }
    }

    fn cmd_echo(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut do_interpret_escapes = false;
        let mut no_newline = false;
        let mut text_args = Vec::new();
        
        for arg in args {
            match arg.as_str() {
                "-e" => do_interpret_escapes = true,
                "-n" => no_newline = true,
                "-en" | "-ne" => {
                    do_interpret_escapes = true;
                    no_newline = true;
                }
                _ => text_args.push(arg.as_str()),
            }
        }
        
        let mut output = text_args.join(" ");
        
        if do_interpret_escapes {
            output = interpret_escape_sequences(&output);
        }
        
        if no_newline {
            ctx.stdout.write(output.as_bytes()).map_err(Sh9Error::Io)?;
        } else {
            ctx.stdout.writeln(&output).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    fn cmd_printf(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            return Ok(0);
        }
        let format_str = &args[0];
        let format_args = &args[1..];
        let mut result = String::new();
        let mut chars = format_str.chars().peekable();
        let mut arg_idx = 0;

        while let Some(c) = chars.next() {
            if c == '%' {
                match chars.peek() {
                    Some('s') => {
                        chars.next();
                        if arg_idx < format_args.len() {
                            result.push_str(&format_args[arg_idx]);
                            arg_idx += 1;
                        }
                    }
                    Some('d') => {
                        chars.next();
                        if arg_idx < format_args.len() {
                            let n: i64 = format_args[arg_idx].parse().unwrap_or(0);
                            result.push_str(&n.to_string());
                            arg_idx += 1;
                        }
                    }
                    Some('%') => {
                        chars.next();
                        result.push('%');
                    }
                    _ => result.push('%'),
                }
            } else if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
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
        ctx.stdout.write(result.as_bytes()).map_err(Sh9Error::Io)?;
        Ok(0)
    }

    async fn cmd_cat(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_grep(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
        
        Ok(if found_any { 0 } else { 1 })
    }

    fn cmd_wc(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_head(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    async fn cmd_tail(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut n: usize = 10;
        let mut follow = false;
        let mut paths: Vec<String> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "-n" && i + 1 < args.len() {
                n = args[i + 1].parse().unwrap_or(10);
                i += 2;
            } else if arg.starts_with("-n") && arg.len() > 2 {
                n = arg[2..].parse().unwrap_or(10);
                i += 1;
            } else if arg.starts_with('-') && arg.len() > 1 && arg[1..].chars().all(|c| c.is_ascii_digit()) {
                n = arg[1..].parse().unwrap_or(10);
                i += 1;
            } else if arg == "-f" || arg == "--follow" {
                follow = true;
                i += 1;
            } else if !arg.starts_with('-') {
                paths.push(arg.clone());
                i += 1;
            } else {
                i += 1;
            }
        }

        if paths.is_empty() {
            let input = ctx.stdin.take().unwrap_or_default();
            let input_str = String::from_utf8_lossy(&input);
            let lines: Vec<&str> = input_str.lines().collect();
            let start = lines.len().saturating_sub(n);

            for line in &lines[start..] {
                ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
            }
            return Ok(0);
        }

        for path in &paths {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                match client.open(&full_path, OpenFlags::read()).await {
                    Ok(handle) => {
                        let mut content = Vec::new();
                        let mut offset = 0u64;

                        loop {
                            match client.read(&handle, offset, STREAM_CHUNK_SIZE).await {
                                Ok(data) if data.is_empty() => break,
                                Ok(data) => {
                                    content.extend_from_slice(&data);
                                    offset += data.len() as u64;
                                }
                                Err(e) => {
                                    let _ = client.close(handle).await;
                                    ctx.write_err(&format!("tail: {}: {}", path, e));
                                    return Ok(1);
                                }
                            }
                        }

                        let content_str = String::from_utf8_lossy(&content);
                        let all_lines: Vec<&str> = content_str.lines().collect();
                        let start = all_lines.len().saturating_sub(n);

                        for line in &all_lines[start..] {
                            ctx.stdout.writeln(line).map_err(Sh9Error::Io)?;
                        }

                        if follow {
                            loop {
                                match client.read(&handle, offset, STREAM_CHUNK_SIZE).await {
                                    Ok(data) if data.is_empty() => {
                                        ctx.stdout.flush().await.map_err(Sh9Error::Io)?;
                                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                        continue;
                                    }
                                    Ok(data) => {
                                        ctx.stdout.write(&data).map_err(Sh9Error::Io)?;
                                        offset += data.len() as u64;
                                    }
                                    Err(_) => {
                                        ctx.stdout.flush().await.map_err(Sh9Error::Io)?;
                                        let _ = client.close(handle).await;
                                        break;
                                    }
                                }
                            }
                        } else {
                            let _ = client.close(handle).await;
                        }
                    }
                    Err(e) => {
                        ctx.write_err(&format!("tail: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("tail: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_sort(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let reverse = args.iter().any(|a| a == "-r");
        let input = if let Some(data) = ctx.stdin.take() {
            String::from_utf8_lossy(&data).to_string()
        } else if let Some(path) = args.iter().find(|a| !a.starts_with('-')) {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                match client.read_file(&full_path).await {
                    Ok(data) => String::from_utf8_lossy(&data).to_string(),
                    Err(e) => {
                        ctx.write_err(&format!("sort: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("sort: not connected to FS9 server");
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

    async fn cmd_uniq(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let input = if let Some(data) = ctx.stdin.take() {
            String::from_utf8_lossy(&data).to_string()
        } else if let Some(path) = args.first() {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                match client.read_file(&full_path).await {
                    Ok(data) => String::from_utf8_lossy(&data).to_string(),
                    Err(e) => {
                        ctx.write_err(&format!("uniq: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("uniq: not connected to FS9 server");
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

    fn cmd_tr(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
            ctx.write_err("tr: missing operand");
            return Ok(1);
        }
        
        let delete_mode = args.first().map(|s| s == "-d").unwrap_or(false);
        let (set1, set2) = if delete_mode {
            if args.len() < 2 {
                ctx.write_err("tr: missing operand");
                return Ok(1);
            }
            (&args[1], None)
        } else {
            if args.len() < 2 {
                ctx.write_err("tr: missing operand");
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

    async fn cmd_rev(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let input = if let Some(data) = ctx.stdin.take() {
            String::from_utf8_lossy(&data).to_string()
        } else if let Some(path) = args.first() {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                match client.read_file(&full_path).await {
                    Ok(data) => String::from_utf8_lossy(&data).to_string(),
                    Err(e) => {
                        ctx.write_err(&format!("rev: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("rev: not connected to FS9 server");
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

    async fn cmd_cut(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    async fn cmd_tee(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_jq(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_date(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_seq(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let (start, end) = match args.len() {
            1 => (1i64, args[0].parse::<i64>().unwrap_or(1)),
            2 => (
                args[0].parse::<i64>().unwrap_or(1),
                args[1].parse::<i64>().unwrap_or(1),
            ),
            _ => {
                ctx.write_err("seq: requires 1 or 2 arguments");
                return Ok(1);
            }
        };
        for i in start..=end {
            ctx.stdout.writeln(&i.to_string()).map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }

    fn cmd_read(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let var_name = args.first().map(|s| s.as_str()).unwrap_or("REPLY");
        if let Some(data) = ctx.stdin.take() {
            let value = String::from_utf8_lossy(&data).trim_end_matches('\n').to_string();
            if ctx.locals.contains_key(var_name) {
                ctx.locals.insert(var_name.to_string(), value);
            } else {
                self.set_var(var_name, &value);
            }
            Ok(0)
        } else {
            Ok(1)
        }
    }
}
