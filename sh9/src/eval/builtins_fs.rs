use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;
use fs9_client::OpenFlags;
use super::{ExecContext, STREAM_CHUNK_SIZE};
use super::utils::format_mtime;

impl Shell {
    pub(crate) async fn try_execute_fs_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Option<Sh9Result<i32>> {
        match name {
            "ls" | "mkdir" | "touch" | "truncate" | "rm" | "mv" | "cp" | "stat"
            | "mount" | "lsfs" | "tree" | "plugin" | "chmod" | "chroot"
            | "basename" | "dirname" | "pwd" | "cd" => {
                Some(self.dispatch_fs_builtin(name, args, ctx).await)
            }
            _ => None,
        }
    }

    async fn dispatch_fs_builtin(
        &mut self,
        name: &str,
        args: &[String],
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        match name {
            "pwd" => self.cmd_pwd(ctx),
            "cd" => self.cmd_cd(args, ctx).await,
            "ls" => self.cmd_ls(args, ctx).await,
            "mkdir" => self.cmd_mkdir(args, ctx).await,
            "touch" => self.cmd_touch(args, ctx).await,
            "truncate" => self.cmd_truncate(args, ctx).await,
            "rm" => self.cmd_rm(args, ctx).await,
            "mv" => self.cmd_mv(args, ctx).await,
            "cp" => self.cmd_cp(args, ctx).await,
            "stat" => self.cmd_stat(args, ctx).await,
            "mount" => {
                ctx.write_err("mount: command disabled for security reasons");
                Ok(1)
            },
            "lsfs" => {
                ctx.write_err("lsfs: command disabled for security reasons");
                Ok(1)
            },
            "tree" => self.cmd_tree(args, ctx).await,
            "plugin" => {
                ctx.write_err("plugin: command disabled for security reasons");
                Ok(1)
            },
            "chroot" => self.cmd_chroot(args, ctx).await,
            "basename" => self.cmd_basename(args, ctx),
            "dirname" => self.cmd_dirname(args, ctx),
            _ => unreachable!(),
        }
    }

    fn cmd_pwd(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        ctx.stdout.writeln(&self.cwd).map_err(Sh9Error::Io)?;
        Ok(0)
    }

    async fn cmd_cd(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let path = args.first().map(|s| s.as_str()).unwrap_or("/");
        let new_cwd = self.resolve_path(path);
        
        if let Some(client) = &self.client {
            match client.stat(&new_cwd).await {
                Ok(stat) if stat.is_dir() => {
                    self.cwd = new_cwd;
                    Ok(0)
                }
                Ok(_) => {
                    ctx.write_err(&format!("cd: {}: Not a directory", path));
                    Ok(1)
                }
                Err(_) => {
                    ctx.write_err(&format!("cd: {}: No such file or directory", path));
                    Ok(1)
                }
            }
        } else {
            self.cwd = new_cwd;
            Ok(0)
        }
    }

    async fn cmd_ls(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut long_format = false;
        let mut recursive = false;
        let mut path = ".";

        for arg in args {
            match arg.as_str() {
                "-l" => long_format = true,
                "-la" | "-al" => long_format = true,
                "-R" => recursive = true,
                "-lR" | "-Rl" => { long_format = true; recursive = true; }
                s if s.starts_with('-') => {
                    for c in s[1..].chars() {
                        match c {
                            'l' => long_format = true,
                            'a' => {} // -a is accepted but all entries are already shown
                            'R' => recursive = true,
                            _ => {}
                        }
                    }
                }
                s => path = s,
            }
        }

        let full_path = self.resolve_path(path);

        if let Some(client) = &self.client {
            // Check if path is a file (not a directory)
            match client.stat(&full_path).await {
                Ok(stat) if !stat.is_dir() => {
                    // It's a file — show it as a single entry
                    if long_format {
                        let type_char = '-';
                        let mode = stat.mode;
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
                        let mtime_str = format_mtime(stat.mtime);
                        let line = format!(
                            "{}{} {} {} {:>6} {} {}",
                            type_char, mode_str, stat.uid, stat.gid, stat.size, mtime_str, path
                        );
                        ctx.stdout.writeln(&line).map_err(Sh9Error::Io)?;
                    } else {
                        ctx.stdout.writeln(path).map_err(Sh9Error::Io)?;
                    }
                    return Ok(0);
                }
                Err(e) => {
                    ctx.write_err(&format!("ls: {}: {}", path, e));
                    return Ok(1);
                }
                _ => {} // It's a directory, continue below
            }

            self.ls_dir(client, &full_path, path, long_format, recursive, ctx).await
        } else {
            ctx.write_err("ls: not connected to FS9 server");
            Ok(1)
        }
    }

    async fn ls_dir(
        &self,
        client: &fs9_client::Fs9Client,
        full_path: &str,
        display_path: &str,
        long_format: bool,
        recursive: bool,
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        match client.readdir(full_path).await {
            Ok(entries) => {
                let mut subdirs = Vec::new();
                for entry in &entries {
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
                            type_char, mode_str, entry.uid, entry.gid, entry.size, mtime_str, entry.name()
                        );
                        ctx.stdout.writeln(&line).map_err(Sh9Error::Io)?;
                    } else {
                        ctx.stdout.writeln(entry.name()).map_err(Sh9Error::Io)?;
                    }
                    if recursive && entry.is_dir() {
                        let sub_full = if full_path.ends_with('/') {
                            format!("{}{}", full_path, entry.name())
                        } else {
                            format!("{}/{}", full_path, entry.name())
                        };
                        let sub_display = if display_path == "." {
                            entry.name().to_string()
                        } else {
                            format!("{}/{}", display_path, entry.name())
                        };
                        subdirs.push((sub_full, sub_display));
                    }
                }
                // Recurse into subdirectories
                for (sub_full, sub_display) in subdirs {
                    ctx.stdout.writeln("").map_err(Sh9Error::Io)?;
                    ctx.stdout.writeln(&format!("{}:", sub_display)).map_err(Sh9Error::Io)?;
                    Box::pin(self.ls_dir(client, &sub_full, &sub_display, long_format, recursive, ctx)).await?;
                }
                Ok(0)
            }
            Err(e) => {
                ctx.write_err(&format!("ls: {}: {}", display_path, e));
                Ok(1)
            }
        }
    }

    async fn cmd_mkdir(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut create_parents = false;
        let mut paths: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-p" => create_parents = true,
                s if s.starts_with('-') => {
                    // Handle combined flags like -pv
                    for c in s[1..].chars() {
                        if c == 'p' { create_parents = true; }
                    }
                }
                s => paths.push(s),
            }
        }

        for path in paths {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                if create_parents {
                    // Create each component of the path
                    let parts: Vec<&str> = full_path.split('/').filter(|s| !s.is_empty()).collect();
                    let mut current = String::new();
                    for part in parts {
                        current = format!("{}/{}", current, part);
                        // Try to create, ignore "already exists" errors
                        let _ = client.mkdir(&current).await;
                    }
                } else if let Err(e) = client.mkdir(&full_path).await {
                    ctx.write_err(&format!("mkdir: {}: {}", path, e));
                    return Ok(1);
                }
            } else {
                ctx.write_err("mkdir: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_touch(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        for path in args {
            if path.starts_with('-') {
                continue;
            }
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                // Check if file already exists — if so, leave it alone
                match client.stat(&full_path).await {
                    Ok(_) => {
                        // File exists, nothing to do (no utimes API available)
                    }
                    Err(_) => {
                        // File doesn't exist, create empty file
                        if let Err(e) = client.write_file(&full_path, &[]).await {
                            ctx.write_err(&format!("touch: {}: {}", path, e));
                            return Ok(1);
                        }
                    }
                }
            } else {
                ctx.write_err("touch: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_truncate(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
                    ctx.write_err(&format!("truncate: {}: {}", path, e));
                    return Ok(1);
                }
            } else {
                ctx.write_err("truncate: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_rm(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut recursive = false;
        let mut force = false;
        let mut paths: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-r" | "-R" => recursive = true,
                "-f" => force = true,
                "-rf" | "-fr" | "-Rf" | "-fR" => { recursive = true; force = true; }
                s if s.starts_with('-') => {
                    for c in s[1..].chars() {
                        match c {
                            'r' | 'R' => recursive = true,
                            'f' => force = true,
                            _ => {}
                        }
                    }
                }
                s => paths.push(s),
            }
        }

        for path in paths {
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                if recursive {
                    if let Err(e) = self.rm_recursive(client, &full_path).await {
                        if !force {
                            ctx.write_err(&format!("rm: {}: {}", path, e));
                            return Ok(1);
                        }
                    }
                } else if let Err(e) = client.remove(&full_path).await {
                    if !force {
                        ctx.write_err(&format!("rm: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("rm: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn rm_recursive(&self, client: &fs9_client::Fs9Client, path: &str) -> Result<(), fs9_client::Fs9Error> {
        match client.stat(path).await {
            Ok(stat) if stat.is_dir() => {
                // List directory entries and remove them first
                let entries = client.readdir(path).await?;
                for entry in entries {
                    let child_path = if path.ends_with('/') {
                        format!("{}{}", path, entry.name())
                    } else {
                        format!("{}/{}", path, entry.name())
                    };
                    // Use Box::pin for recursive async call
                    Box::pin(self.rm_recursive(client, &child_path)).await?;
                }
                // Now remove the empty directory
                client.remove(path).await
            }
            Ok(_) => {
                // It's a file, just remove it
                client.remove(path).await
            }
            Err(e) => Err(e),
        }
    }

    async fn cmd_mv(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.len() != 2 {
            ctx.write_err("mv: requires two arguments");
            return Ok(1);
        }
        let src = self.resolve_path(&args[0]);
        let dst = self.resolve_path(&args[1]);
        
        if let Some(client) = &self.client {
            if let Err(e) = client.rename(&src, &dst).await {
                ctx.write_err(&format!("mv: {}", e));
                return Ok(1);
            }
            Ok(0)
        } else {
            ctx.write_err("mv: not connected to FS9 server");
            Ok(1)
        }
    }

    async fn cmd_cp(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut recursive = false;
        let mut paths: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-r" | "-R" => recursive = true,
                s if s.starts_with('-') => {
                    for c in s[1..].chars() {
                        if c == 'r' || c == 'R' { recursive = true; }
                    }
                }
                s => paths.push(s),
            }
        }

        if paths.len() != 2 {
            ctx.write_err("cp: requires source and destination arguments");
            return Ok(1);
        }
        let src = self.resolve_path(paths[0]);
        let dst = self.resolve_path(paths[1]);

        if let Some(client) = &self.client {
            // Check if source is a directory
            match client.stat(&src).await {
                Ok(stat) if stat.is_dir() => {
                    if !recursive {
                        ctx.write_err(&format!("cp: -r not specified; omitting directory '{}'", paths[0]));
                        return Ok(1);
                    }
                    // Recursive directory copy
                    if let Err(e) = self.cp_recursive(client, &src, &dst).await {
                        ctx.write_err(&format!("cp: {}", e));
                        return Ok(1);
                    }
                    Ok(0)
                }
                Ok(_) => {
                    // File copy
                    self.cp_file(client, &src, &dst, paths[0], paths[1], ctx).await
                }
                Err(e) => {
                    ctx.write_err(&format!("cp: {}: {}", paths[0], e));
                    Ok(1)
                }
            }
        } else {
            ctx.write_err("cp: not connected to FS9 server");
            Ok(1)
        }
    }

    async fn cp_file(
        &self,
        client: &fs9_client::Fs9Client,
        src: &str,
        dst: &str,
        src_display: &str,
        dst_display: &str,
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        let src_handle = match client.open(src, OpenFlags::read()).await {
            Ok(h) => h,
            Err(e) => {
                ctx.write_err(&format!("cp: {}: {}", src_display, e));
                return Ok(1);
            }
        };
        let dst_handle = match client.open(dst, OpenFlags::create_truncate()).await {
            Ok(h) => h,
            Err(e) => {
                let _ = client.close(src_handle).await;
                ctx.write_err(&format!("cp: {}: {}", dst_display, e));
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
                        ctx.write_err(&format!("cp: write error: {}", e));
                        return Ok(1);
                    }
                    offset += len as u64;
                }
                Err(e) => {
                    let _ = client.close(src_handle).await;
                    let _ = client.close(dst_handle).await;
                    ctx.write_err(&format!("cp: read error: {}", e));
                    return Ok(1);
                }
            }
        }
        let _ = client.close(src_handle).await;
        let _ = client.close(dst_handle).await;
        Ok(0)
    }

    async fn cp_recursive(
        &self,
        client: &fs9_client::Fs9Client,
        src: &str,
        dst: &str,
    ) -> Result<(), fs9_client::Fs9Error> {
        // Create destination directory
        let _ = client.mkdir(dst).await;

        let entries = client.readdir(src).await?;
        for entry in entries {
            let src_child = if src.ends_with('/') {
                format!("{}{}", src, entry.name())
            } else {
                format!("{}/{}", src, entry.name())
            };
            let dst_child = if dst.ends_with('/') {
                format!("{}{}", dst, entry.name())
            } else {
                format!("{}/{}", dst, entry.name())
            };

            if entry.is_dir() {
                Box::pin(self.cp_recursive(client, &src_child, &dst_child)).await?;
            } else {
                // Copy file contents
                let data = client.read_file(&src_child).await?;
                client.write_file(&dst_child, &data).await?;
            }
        }
        Ok(())
    }

    async fn cmd_stat(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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
                        ctx.write_err(&format!("stat: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else {
                ctx.write_err("stat: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
    }

    // cmd_mount, cmd_lsfs removed — disabled for security reasons

    async fn cmd_tree(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    // cmd_plugin removed — disabled for security reasons

    async fn cmd_chroot(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
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

    fn cmd_basename(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            ctx.write_err("basename: missing operand");
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

    fn cmd_dirname(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            ctx.write_err("dirname: missing operand");
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
}
