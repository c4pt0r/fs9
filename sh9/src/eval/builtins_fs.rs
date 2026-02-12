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
            "mount" => self.cmd_mount(args, ctx).await,
            "lsfs" => self.cmd_lsfs(ctx).await,
            "tree" => self.cmd_tree(args, ctx).await,
            "plugin" => self.cmd_plugin(args, ctx).await,
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

    async fn cmd_mkdir(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        for path in args {
            if path.starts_with('-') {
                continue;
            }
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                if let Err(e) = client.mkdir(&full_path).await {
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
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                if let Err(e) = client.write_file(&full_path, &[]).await {
                    ctx.write_err(&format!("touch: {}: {}", path, e));
                    return Ok(1);
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
        for path in args {
            if path.starts_with('-') {
                continue;
            }
            let full_path = self.resolve_path(path);
            if let Some(client) = &self.client {
                if let Err(e) = client.remove(&full_path).await {
                    ctx.write_err(&format!("rm: {}: {}", path, e));
                    return Ok(1);
                }
            } else {
                ctx.write_err("rm: not connected to FS9 server");
                return Ok(1);
            }
        }
        Ok(0)
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
        if args.len() != 2 {
            ctx.write_err("cp: requires two arguments");
            return Ok(1);
        }
        let src = self.resolve_path(&args[0]);
        let dst = self.resolve_path(&args[1]);
        
        if let Some(client) = &self.client {
            let src_handle = match client.open(&src, OpenFlags::read()).await {
                Ok(h) => h,
                Err(e) => {
                    ctx.write_err(&format!("cp: {}: {}", args[0], e));
                    return Ok(1);
                }
            };
            let dst_handle = match client.open(&dst, OpenFlags::create_truncate()).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = client.close(src_handle).await;
                    ctx.write_err(&format!("cp: {}: {}", args[1], e));
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
        } else {
            ctx.write_err("cp: not connected to FS9 server");
            Ok(1)
        }
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

    async fn cmd_mount(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if let Some(client) = &self.client {
            if args.is_empty() {
                match client.list_mounts().await {
                    Ok(mounts) => {
                        for m in mounts {
                            ctx.stdout.writeln(&format!("{:<20} {}", m.path, m.provider_name)).map_err(Sh9Error::Io)?;
                        }
                        Ok(0)
                    }
                    Err(e) => {
                        ctx.write_err(&format!("mount: {}", e));
                        Ok(1)
                    }
                }
            } else if args.len() >= 2 {
                let provider = &args[0];
                let mount_path = &args[1];
                let config: Option<serde_json::Value> = if args.len() > 2 {
                    match serde_json::from_str(&args[2]) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            ctx.write_err(&format!("mount: invalid config JSON: {}", e));
                            return Ok(1);
                        }
                    }
                } else {
                    None
                };

                match client.mount_plugin(mount_path, provider, config).await {
                    Ok(info) => {
                        ctx.stdout.writeln(&format!("mounted {} at {}", info.provider_name, info.path)).map_err(Sh9Error::Io)?;
                        Ok(0)
                    }
                    Err(e) => {
                        ctx.write_err(&format!("mount: {}", e));
                        Ok(1)
                    }
                }
            } else {
                ctx.write_err("mount: usage: mount [<fstype> <mount_point> [config_json]]");
                Ok(1)
            }
        } else {
            ctx.write_err("mount: not connected to FS9 server");
            Ok(1)
        }
    }

    async fn cmd_lsfs(&mut self, ctx: &mut ExecContext) -> Sh9Result<i32> {
        if let Some(client) = &self.client {
            let plugins = match client.list_plugins().await {
                Ok(p) => p,
                Err(e) => {
                    ctx.write_err(&format!("lsfs: {}", e));
                    return Ok(1);
                }
            };

            let mounts = match client.list_mounts().await {
                Ok(m) => m,
                Err(e) => {
                    ctx.write_err(&format!("lsfs: {}", e));
                    return Ok(1);
                }
            };

            ctx.stdout.writeln("Available filesystems:").map_err(Sh9Error::Io)?;
            if plugins.is_empty() {
                ctx.stdout.writeln("  (none)").map_err(Sh9Error::Io)?;
            } else {
                for plugin in &plugins {
                    ctx.stdout.writeln(&format!("  {}", plugin)).map_err(Sh9Error::Io)?;
                }
            }

            ctx.stdout.writeln("").map_err(Sh9Error::Io)?;
            ctx.stdout.writeln("Current mounts:").map_err(Sh9Error::Io)?;
            if mounts.is_empty() {
                ctx.stdout.writeln("  (none)").map_err(Sh9Error::Io)?;
            } else {
                for m in &mounts {
                    ctx.stdout.writeln(&format!("  {:<20} {}", m.path, m.provider_name)).map_err(Sh9Error::Io)?;
                }
            }

            Ok(0)
        } else {
            ctx.write_err("lsfs: not connected to FS9 server");
            Ok(1)
        }
    }

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

    async fn cmd_plugin(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if let Some(client) = &self.client {
            if args.is_empty() {
                ctx.write_err("plugin: usage: plugin <load|unload|list> [args...]");
                return Ok(1);
            }

            match args[0].as_str() {
                "list" => {
                    match client.list_plugins().await {
                        Ok(plugins) => {
                            if plugins.is_empty() {
                                ctx.stdout.writeln("no plugins loaded").map_err(Sh9Error::Io)?;
                            } else {
                                for p in plugins {
                                    ctx.stdout.writeln(&p).map_err(Sh9Error::Io)?;
                                }
                            }
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("plugin list: {}", e));
                            Ok(1)
                        }
                    }
                }
                "load" => {
                    if args.len() < 3 {
                        ctx.write_err("plugin load: usage: plugin load <name> <path>");
                        return Ok(1);
                    }
                    let name = &args[1];
                    let path = &args[2];
                    match client.load_plugin(name, path).await {
                        Ok(info) => {
                            ctx.stdout.writeln(&format!("loaded plugin '{}': {}", info.name, info.status)).map_err(Sh9Error::Io)?;
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("plugin load: {}", e));
                            Ok(1)
                        }
                    }
                }
                "unload" => {
                    if args.len() < 2 {
                        ctx.write_err("plugin unload: usage: plugin unload <name>");
                        return Ok(1);
                    }
                    let name = &args[1];
                    match client.unload_plugin(name).await {
                        Ok(()) => {
                            ctx.stdout.writeln(&format!("unloaded plugin '{}'", name)).map_err(Sh9Error::Io)?;
                            Ok(0)
                        }
                        Err(e) => {
                            ctx.write_err(&format!("plugin unload: {}", e));
                            Ok(1)
                        }
                    }
                }
                _ => {
                    ctx.write_err(&format!("plugin: unknown subcommand '{}'. Use: load, unload, list", args[0]));
                    Ok(1)
                }
            }
        } else {
            ctx.write_err("plugin: not connected to FS9 server");
            Ok(1)
        }
    }

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
