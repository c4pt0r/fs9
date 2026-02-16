use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;
use super::ExecContext;
use super::namespace::MountFlags;
use super::router::NamespaceRouter;
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
            | "basename" | "dirname" | "pwd" | "cd"
            | "bind" | "unmount" | "ns" => {
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
            "chmod" => self.cmd_chmod(args, ctx).await,
            "chroot" => self.cmd_chroot(args, ctx).await,
            "basename" => self.cmd_basename(args, ctx),
            "dirname" => self.cmd_dirname(args, ctx),
            "bind" => self.cmd_bind(args, ctx),
            "unmount" => self.cmd_unmount(args, ctx),
            "ns" => self.cmd_ns(args, ctx),
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

        let router = self.router();
        if router.has_client() || router.is_local(&new_cwd) {
            match router.stat(&new_cwd).await {
                Ok(info) if info.is_dir => {
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
        let router = self.router();

        // Check if path is a file (not a directory)
        match router.stat(&full_path).await {
            Ok(info) if !info.is_dir => {
                // It's a file — show it as a single entry
                if long_format {
                    let type_char = '-';
                    let mode = info.mode;
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
                    let mtime_str = format_mtime(info.mtime);
                    let line = format!(
                        "{}{} {} {} {:>6} {} {}",
                        type_char, mode_str, info.uid, info.gid, info.size, mtime_str, path
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

        self.ls_dir(&router, &full_path, path, long_format, recursive, ctx).await
    }

    async fn ls_dir(
        &self,
        router: &NamespaceRouter,
        full_path: &str,
        display_path: &str,
        long_format: bool,
        recursive: bool,
        ctx: &mut ExecContext,
    ) -> Sh9Result<i32> {
        match router.readdir(full_path).await {
            Ok(entries) => {
                let mut subdirs = Vec::new();
                for entry in &entries {
                    if long_format {
                        let type_char = if entry.is_dir { 'd' } else { '-' };
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
                            type_char, mode_str, entry.uid, entry.gid, entry.size, mtime_str, entry.name
                        );
                        ctx.stdout.writeln(&line).map_err(Sh9Error::Io)?;
                    } else {
                        ctx.stdout.writeln(&entry.name).map_err(Sh9Error::Io)?;
                    }
                    if recursive && entry.is_dir {
                        let sub_full = if full_path.ends_with('/') {
                            format!("{}{}", full_path, entry.name)
                        } else {
                            format!("{}/{}", full_path, entry.name)
                        };
                        let sub_display = if display_path == "." {
                            entry.name.clone()
                        } else {
                            format!("{}/{}", display_path, entry.name)
                        };
                        subdirs.push((sub_full, sub_display));
                    }
                }
                // Recurse into subdirectories
                for (sub_full, sub_display) in subdirs {
                    ctx.stdout.writeln("").map_err(Sh9Error::Io)?;
                    ctx.stdout.writeln(&format!("{}:", sub_display)).map_err(Sh9Error::Io)?;
                    Box::pin(self.ls_dir(router, &sub_full, &sub_display, long_format, recursive, ctx)).await?;
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

        let router = self.router();
        for path in paths {
            let full_path = self.resolve_path(path);
            if create_parents {
                // Create each component of the path
                let parts: Vec<&str> = full_path.split('/').filter(|s| !s.is_empty()).collect();
                let mut current = String::new();
                for part in parts {
                    current = format!("{}/{}", current, part);
                    // Try to create, ignore "already exists" errors
                    let _ = router.mkdir(&current).await;
                }
            } else if let Err(e) = router.mkdir(&full_path).await {
                ctx.write_err(&format!("mkdir: {}: {}", path, e));
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_touch(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let router = self.router();
        for path in args {
            if path.starts_with('-') {
                continue;
            }
            let full_path = self.resolve_path(path);
            // Check if file already exists — if so, leave it alone
            match router.stat(&full_path).await {
                Ok(_) => {
                    // File exists, nothing to do (no utimes API available)
                }
                Err(_) => {
                    // File doesn't exist, create empty file
                    if let Err(e) = router.write_file(&full_path, &[]).await {
                        ctx.write_err(&format!("touch: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            }
        }
        Ok(0)
    }

    async fn cmd_truncate(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut size: Option<u64> = None;
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
        let router = self.router();

        for path in files {
            let full_path = self.resolve_path(path);
            if let Err(e) = router.truncate(&full_path, target_size).await {
                ctx.write_err(&format!("truncate: {}: {}", path, e));
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

        let router = self.router();
        for path in paths {
            let full_path = self.resolve_path(path);

            // Check if the path is a mount point
            let mounts = router.namespace.list_mounts();
            if mounts.iter().any(|m| m.target == full_path) {
                ctx.write_err(&format!("rm: {}: cannot remove mount point; use unmount", path));
                return Ok(1);
            }

            if recursive {
                if let Err(e) = router.remove_recursive(&full_path).await {
                    if !force {
                        ctx.write_err(&format!("rm: {}: {}", path, e));
                        return Ok(1);
                    }
                }
            } else if let Err(e) = router.remove(&full_path).await {
                if !force {
                    ctx.write_err(&format!("rm: {}: {}", path, e));
                    return Ok(1);
                }
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

        let router = self.router();
        let effective_dst = match router.stat(&dst).await {
            Ok(info) if info.is_dir => {
                let basename = src.rsplit('/').next().unwrap_or(&src);
                format!("{}/{}", dst.trim_end_matches('/'), basename)
            }
            _ => dst.clone(),
        };
        match router.rename(&src, &effective_dst).await {
            Ok(()) => Ok(0),
            Err(e) if e.contains("cross-mount rename") => {
                if let Err(e) = router.copy(&src, &effective_dst).await {
                    ctx.write_err(&format!("mv: {}", e));
                    return Ok(1);
                }
                if let Err(e) = router.remove(&src).await {
                    ctx.write_err(&format!("mv: {}", e));
                    return Ok(1);
                }
                Ok(0)
            }
            Err(e) => {
                ctx.write_err(&format!("mv: {}", e));
                Ok(1)
            }
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

        let router = self.router();
        match router.stat(&src).await {
            Ok(info) if info.is_dir => {
                if !recursive {
                    ctx.write_err(&format!("cp: -r not specified; omitting directory '{}'", paths[0]));
                    return Ok(1);
                }
                if let Err(e) = self.cp_recursive_via_router(&router, &src, &dst).await {
                    ctx.write_err(&format!("cp: {}", e));
                    return Ok(1);
                }
                Ok(0)
            }
            Ok(_) => {
                let effective_dst = match router.stat(&dst).await {
                    Ok(info) if info.is_dir => {
                        let basename = src.rsplit('/').next().unwrap_or(&src);
                        format!("{}/{}", dst.trim_end_matches('/'), basename)
                    }
                    _ => dst.clone(),
                };
                if let Err(e) = router.copy(&src, &effective_dst).await {
                    ctx.write_err(&format!("cp: {}", e));
                    return Ok(1);
                }
                Ok(0)
            }
            Err(e) => {
                ctx.write_err(&format!("cp: {}: {}", paths[0], e));
                Ok(1)
            }
        }
    }

    async fn cp_recursive_via_router(
        &self,
        router: &NamespaceRouter,
        src: &str,
        dst: &str,
    ) -> Result<(), String> {
        let _ = router.mkdir(dst).await;

        let entries = router.readdir(src).await?;
        for entry in entries {
            let src_child = if src == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", src, entry.name)
            };
            let dst_child = if dst == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", dst, entry.name)
            };

            if entry.is_dir {
                Box::pin(self.cp_recursive_via_router(router, &src_child, &dst_child)).await?;
            } else {
                router.copy(&src_child, &dst_child).await?;
            }
        }
        Ok(())
    }

    async fn cmd_stat(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let router = self.router();
        for path in args {
            let full_path = self.resolve_path(path);
            match router.stat(&full_path).await {
                Ok(info) => {
                    ctx.stdout.writeln(&format!("file: {}", path)).map_err(Sh9Error::Io)?;
                    ctx.stdout.writeln(&format!("size: {}", info.size)).map_err(Sh9Error::Io)?;
                    ctx.stdout.writeln(&format!("type: {}", if info.is_dir { "directory" } else { "file" })).map_err(Sh9Error::Io)?;
                }
                Err(e) => {
                    ctx.write_err(&format!("stat: {}: {}", path, e));
                    return Ok(1);
                }
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

        let router = self.router();
        self.print_tree(&router, &full_path, "", true, 0, max_depth, dirs_only, show_hidden, ctx).await?;
        Ok(0)
    }

    // cmd_plugin removed — disabled for security reasons

    async fn cmd_chmod(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.len() < 2 {
            ctx.write_err("chmod: usage: chmod MODE FILE...");
            return Ok(1);
        }

        let mode_str = &args[0];
        let mode = match u32::from_str_radix(mode_str, 8) {
            Ok(m) => m,
            Err(_) => {
                ctx.write_err(&format!("chmod: invalid mode: {}", mode_str));
                return Ok(1);
            }
        };

        let router = self.router();
        for path in &args[1..] {
            let full_path = self.resolve_path(path);
            if let Err(e) = router.chmod(&full_path, mode).await {
                ctx.write_err(&format!("chmod: {}: {}", path, e));
                return Ok(1);
            }
        }
        Ok(0)
    }

    async fn cmd_chroot(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        if args.is_empty() {
            ctx.stdout.writeln(&format!("Current root: {}", self.get_var("FS9_CHROOT").unwrap_or("/"))).map_err(Sh9Error::Io)?;
        } else if args.first().map(|s| s.as_str()) == Some("--exit") {
            self.env.remove("FS9_CHROOT");
            ctx.stdout.writeln("Exited chroot").map_err(Sh9Error::Io)?;
        } else {
            let new_root = self.resolve_path(&args[0]);
            let router = self.router();
            match router.stat(&new_root).await {
                Ok(info) if info.is_dir => {
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

    fn cmd_bind(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut flags = MountFlags::MREPL;
        let mut positional: Vec<&str> = Vec::new();

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'b' => flags |= MountFlags::MBEFORE,
                        'a' => flags |= MountFlags::MAFTER,
                        'c' => flags |= MountFlags::MCREATE,
                        _ => {
                            ctx.write_err(&format!("bind: unknown option: -{c}"));
                            return Ok(1);
                        }
                    }
                }
            } else {
                positional.push(arg);
            }
        }

        if positional.len() != 2 {
            ctx.write_err("bind: usage: bind [-b|-a] [-c] source target");
            return Ok(1);
        }

        let source_arg = positional[0];
        let target = positional[1];

        let metadata = match std::fs::metadata(source_arg) {
            Ok(m) => m,
            Err(_) => {
                ctx.write_err(&format!("bind: {source_arg}: No such file or directory"));
                return Ok(1);
            }
        };

        if !metadata.is_dir() {
            ctx.write_err(&format!("bind: {source_arg}: Not a directory"));
            return Ok(1);
        }

        let source = match std::fs::canonicalize(source_arg) {
            Ok(p) => p,
            Err(_) => {
                ctx.write_err(&format!("bind: {source_arg}: No such file or directory"));
                return Ok(1);
            }
        };

        let source_str = source.to_string_lossy();
        let target_normalized = super::namespace::normalize_path(target);
        if *source_str == target_normalized {
            ctx.write_err("bind: cannot bind a path to itself");
            return Ok(1);
        }

        self.namespace
            .write()
            .unwrap()
            .bind(&source, &target_normalized, flags);
        ctx.stdout
            .writeln(&format!(
                "bind: {} -> {target_normalized}",
                source.display()
            ))
            .map_err(Sh9Error::Io)?;
        Ok(0)
    }

    fn cmd_unmount(&mut self, args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(|s| s.as_str())
            .collect();

        match positional.len() {
            1 => {
                let target = positional[0];
                let target_normalized = super::namespace::normalize_path(target);

                let is_mounted = self
                    .namespace
                    .read()
                    .unwrap()
                    .list_mounts()
                    .iter()
                    .any(|m| m.target == target_normalized);

                if !is_mounted {
                    ctx.write_err(&format!("unmount: {target}: not mounted"));
                    return Ok(1);
                }

                self.namespace
                    .write()
                    .unwrap()
                    .unbind(None, &target_normalized);
                ctx.stdout
                    .writeln(&format!("unmount: {target_normalized}"))
                    .map_err(Sh9Error::Io)?;
                Ok(0)
            }
            2 => {
                let source = positional[0];
                let target = positional[1];
                let target_normalized = super::namespace::normalize_path(target);

                let source_path = std::fs::canonicalize(source)
                    .unwrap_or_else(|_| std::path::PathBuf::from(source));

                let exists = self
                    .namespace
                    .read()
                    .unwrap()
                    .list_mounts()
                    .iter()
                    .any(|m| m.target == target_normalized && m.source == source_path);

                if !exists {
                    ctx.write_err(&format!("unmount: {source}: not mounted at {target}"));
                    return Ok(1);
                }

                self.namespace
                    .write()
                    .unwrap()
                    .unbind(Some(&source_path), &target_normalized);
                ctx.stdout
                    .writeln(&format!(
                        "unmount: {} from {target_normalized}",
                        source_path.display()
                    ))
                    .map_err(Sh9Error::Io)?;
                Ok(0)
            }
            _ => {
                ctx.write_err("unmount: usage: unmount [source] target");
                Ok(1)
            }
        }
    }

    fn cmd_ns(&mut self, _args: &[String], ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mounts = self.namespace.read().unwrap().list_mounts();

        if mounts.is_empty() {
            ctx.stdout
                .writeln("(no bindings)")
                .map_err(Sh9Error::Io)?;
            return Ok(0);
        }

        for mount in &mounts {
            let flags_str = format_mount_flags(mount.flags);
            ctx.stdout
                .writeln(&format!(
                    "{}\t{}\t({flags_str})",
                    mount.target,
                    mount.source.display(),
                ))
                .map_err(Sh9Error::Io)?;
        }
        Ok(0)
    }
}

fn format_mount_flags(flags: MountFlags) -> String {
    let mut parts = Vec::new();
    if flags.contains(MountFlags::MBEFORE) {
        parts.push("before");
    } else if flags.contains(MountFlags::MAFTER) {
        parts.push("after");
    } else {
        parts.push("replace");
    }
    if flags.contains(MountFlags::MCREATE) {
        parts.push("create");
    }
    parts.join(",")
}
