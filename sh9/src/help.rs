pub struct CommandHelp {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub options: &'static [(&'static str, &'static str)],
}

pub const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        name: "alias",
        summary: "Define or display aliases",
        usage: "alias [name[=value] ...]",
        options: &[],
    },
    CommandHelp {
        name: "basename",
        summary: "Strip directory and suffix from filenames",
        usage: "basename PATH [SUFFIX]",
        options: &[],
    },
    CommandHelp {
        name: "cat",
        summary: "Concatenate and print files",
        usage: "cat [FILE]...",
        options: &[("-", "Read from stdin")],
    },
    CommandHelp {
        name: "cd",
        summary: "Change the current directory",
        usage: "cd [DIR]",
        options: &[],
    },
    CommandHelp {
        name: "cp",
        summary: "Copy files",
        usage: "cp SOURCE DEST",
        options: &[],
    },
    CommandHelp {
        name: "cut",
        summary: "Remove sections from lines",
        usage: "cut [OPTION]... [FILE]",
        options: &[
            ("-d DELIM", "Use DELIM as field delimiter"),
            ("-f FIELDS", "Select only these fields (comma-separated)"),
            ("-c RANGE", "Select only these characters"),
        ],
    },
    CommandHelp {
        name: "date",
        summary: "Display the current date and time",
        usage: "date [+FORMAT]",
        options: &[("+FORMAT", "Output format (e.g., +%Y-%m-%d %H:%M:%S)")],
    },
    CommandHelp {
        name: "dirname",
        summary: "Strip last component from filename",
        usage: "dirname PATH",
        options: &[],
    },
    CommandHelp {
        name: "echo",
        summary: "Display a line of text",
        usage: "echo [-en] [STRING]...",
        options: &[
            ("-e", "Interpret escape sequences (\\n, \\t, etc.)"),
            ("-n", "Do not output trailing newline"),
        ],
    },
    CommandHelp {
        name: "env",
        summary: "Display environment variables",
        usage: "env",
        options: &[],
    },
    CommandHelp {
        name: "exit",
        summary: "Exit the shell",
        usage: "exit [CODE]",
        options: &[],
    },
    CommandHelp {
        name: "export",
        summary: "Set environment variables",
        usage: "export NAME=VALUE...",
        options: &[],
    },
    CommandHelp {
        name: "false",
        summary: "Return failure exit code",
        usage: "false",
        options: &[],
    },
    CommandHelp {
        name: "grep",
        summary: "Search for patterns in files",
        usage: "grep [-E] PATTERN [FILE]",
        options: &[("-E", "Use extended regular expressions")],
    },
    CommandHelp {
        name: "head",
        summary: "Output the first part of files",
        usage: "head [-n NUM] [FILE]",
        options: &[("-n NUM", "Print first NUM lines (default 10)")],
    },
    CommandHelp {
        name: "help",
        summary: "Display help for commands",
        usage: "help [COMMAND]",
        options: &[],
    },
    CommandHelp {
        name: "http",
        summary: "Make HTTP requests",
        usage: "http METHOD URL [BODY]",
        options: &[],
    },
    CommandHelp {
        name: "jobs",
        summary: "List background jobs",
        usage: "jobs",
        options: &[],
    },
    CommandHelp {
        name: "jq",
        summary: "Process JSON data",
        usage: "jq [FILTER]",
        options: &[
            (".", "Output entire JSON"),
            (".field", "Extract field"),
            (".[]", "Iterate array"),
        ],
    },
    CommandHelp {
        name: "local",
        summary: "Declare local variables in functions",
        usage: "local NAME[=VALUE]...",
        options: &[],
    },
    CommandHelp {
        name: "ls",
        summary: "List directory contents",
        usage: "ls [-l] [PATH]",
        options: &[("-l", "Use long listing format")],
    },
    CommandHelp {
        name: "mkdir",
        summary: "Create directories",
        usage: "mkdir DIR...",
        options: &[],
    },
    CommandHelp {
        name: "mount",
        summary: "List mounted filesystems",
        usage: "mount",
        options: &[],
    },
    CommandHelp {
        name: "mv",
        summary: "Move or rename files",
        usage: "mv SOURCE DEST",
        options: &[],
    },
    CommandHelp {
        name: "pwd",
        summary: "Print working directory",
        usage: "pwd",
        options: &[],
    },
    CommandHelp {
        name: "return",
        summary: "Return from a function",
        usage: "return [CODE]",
        options: &[],
    },
    CommandHelp {
        name: "rev",
        summary: "Reverse lines of a file",
        usage: "rev [FILE]",
        options: &[],
    },
    CommandHelp {
        name: "rm",
        summary: "Remove files or directories",
        usage: "rm FILE...",
        options: &[],
    },
    CommandHelp {
        name: "set",
        summary: "Display all shell variables",
        usage: "set",
        options: &[],
    },
    CommandHelp {
        name: "sleep",
        summary: "Delay for a specified time",
        usage: "sleep SECONDS",
        options: &[],
    },
    CommandHelp {
        name: "sort",
        summary: "Sort lines of text",
        usage: "sort [-r] [FILE]",
        options: &[("-r", "Reverse the result of comparisons")],
    },
    CommandHelp {
        name: "source",
        summary: "Execute commands from a file",
        usage: "source FILE [ARGS]...",
        options: &[],
    },
    CommandHelp {
        name: "stat",
        summary: "Display file status",
        usage: "stat FILE...",
        options: &[],
    },
    CommandHelp {
        name: "tail",
        summary: "Output the last part of files",
        usage: "tail [-n NUM] [FILE]",
        options: &[("-n NUM", "Print last NUM lines (default 10)")],
    },
    CommandHelp {
        name: "tee",
        summary: "Read from stdin and write to files",
        usage: "tee [-a] FILE...",
        options: &[("-a", "Append to files instead of overwriting")],
    },
    CommandHelp {
        name: "test",
        summary: "Evaluate conditional expressions",
        usage: "test EXPRESSION  or  [ EXPRESSION ]",
        options: &[
            ("-e FILE", "True if file exists"),
            ("-d FILE", "True if file is a directory"),
            ("-f FILE", "True if file is a regular file"),
            ("-z STRING", "True if string is empty"),
            ("-n STRING", "True if string is not empty"),
            ("S1 = S2", "True if strings are equal"),
            ("S1 != S2", "True if strings are not equal"),
            ("N1 -eq N2", "True if integers are equal"),
            ("N1 -ne N2", "True if integers are not equal"),
            ("N1 -lt N2", "True if N1 < N2"),
            ("N1 -le N2", "True if N1 <= N2"),
            ("N1 -gt N2", "True if N1 > N2"),
            ("N1 -ge N2", "True if N1 >= N2"),
        ],
    },
    CommandHelp {
        name: "touch",
        summary: "Create empty files",
        usage: "touch FILE...",
        options: &[],
    },
    CommandHelp {
        name: "tr",
        summary: "Translate characters",
        usage: "tr [-d] SET1 [SET2]",
        options: &[("-d", "Delete characters in SET1")],
    },
    CommandHelp {
        name: "tree",
        summary: "Display directory tree",
        usage: "tree [-L LEVEL] [-d] [-a] [PATH]",
        options: &[
            ("-L LEVEL", "Limit depth to LEVEL"),
            ("-d", "List directories only"),
            ("-a", "Show hidden files"),
        ],
    },
    CommandHelp {
        name: "true",
        summary: "Return success exit code",
        usage: "true",
        options: &[],
    },
    CommandHelp {
        name: "truncate",
        summary: "Shrink or extend file size",
        usage: "truncate -s SIZE FILE...",
        options: &[("-s SIZE", "Set file size to SIZE bytes")],
    },
    CommandHelp {
        name: "unalias",
        summary: "Remove aliases",
        usage: "unalias NAME...",
        options: &[],
    },
    CommandHelp {
        name: "uniq",
        summary: "Report or omit repeated lines",
        usage: "uniq [FILE]",
        options: &[],
    },
    CommandHelp {
        name: "unset",
        summary: "Unset shell variables",
        usage: "unset NAME...",
        options: &[],
    },
    CommandHelp {
        name: "wait",
        summary: "Wait for background jobs",
        usage: "wait [JOB_ID]",
        options: &[],
    },
    CommandHelp {
        name: "wc",
        summary: "Print line, word, and byte counts",
        usage: "wc [-lwc] [FILE]",
        options: &[
            ("-l", "Print line count"),
            ("-w", "Print word count"),
            ("-c", "Print byte count"),
        ],
    },
];

pub fn get_help(name: &str) -> Option<&'static CommandHelp> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn format_help(cmd: &CommandHelp) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} - {}\n\n", cmd.name, cmd.summary));
    out.push_str(&format!("Usage: {}\n", cmd.usage));
    if !cmd.options.is_empty() {
        out.push_str("\nOptions:\n");
        for (opt, desc) in cmd.options {
            out.push_str(&format!("  {:16} {}\n", opt, desc));
        }
    }
    out
}

pub fn format_help_list() -> String {
    let mut out = String::new();
    out.push_str("sh9 - FS9 Shell Commands\n\n");
    out.push_str("Available commands:\n\n");

    for cmd in COMMANDS {
        out.push_str(&format!("  {:12} {}\n", cmd.name, cmd.summary));
    }

    out.push_str("\nUse 'help COMMAND' or 'COMMAND --help' for more information.\n");
    out
}

pub fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "--help" || a == "-h")
}
