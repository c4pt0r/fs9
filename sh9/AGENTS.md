# sh9 KNOWLEDGE BASE

Bash-like shell for FS9. Lexer → parser → AST → evaluator pipeline. All file operations go through Fs9Client HTTP calls, not direct FS access.

## STRUCTURE

```
sh9/src/
├── main.rs       # CLI entry: REPL or script mode, connects to FS9 server
├── shell.rs      # Shell struct: REPL loop, readline, history, signal handling
├── lexer.rs      # Tokenizer: bash-compatible tokens (454 lines)
├── parser.rs     # Recursive descent parser → AST (557 lines)
├── ast.rs        # AST node types: Command, Pipeline, If, For, While, Function
├── eval.rs       # Evaluator: 3152 lines — ALL built-in commands + control flow + job control
├── completer.rs  # Tab completion for paths and commands
├── help.rs       # Built-in help text for all commands (436 lines)
├── error.rs      # Sh9Error, Sh9Result
└── lib.rs        # Re-exports: Shell, parse, Sh9Error
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add built-in command | `eval.rs` | Find dispatch match, add arm. Update `help.rs` too |
| Fix parsing bug | `parser.rs` → `lexer.rs` | Parser is recursive descent, lexer handles quoting/escaping |
| Modify variable expansion | `eval.rs` | Search for `expand_` functions |
| Change pipeline behavior | `eval.rs` | Search for `eval_pipeline` |
| Add new AST node | `ast.rs` → `parser.rs` → `eval.rs` | Must update all three |
| Job control changes | `eval.rs` | Search for `BackgroundJob`, `jobs`, `fg`, `bg`, `kill` |
| Tab completion | `completer.rs` | Path + command completion logic |

## CONVENTIONS

- **eval.rs is the monolith** — intentional: all commands in one file for grep-ability. Don't split unless >5000 lines
- **Built-in commands** are native Rust, not forked processes. `ls`, `cat`, `grep`, `wc` etc. all implemented in `eval.rs`
- **No `unsafe` code** in sh9 — all FS operations are HTTP calls via `Fs9Client`
- **Test scripts** in `tests/integration/scripts/` (68 `.sh9` files) — run via `cargo test -p sh9`
- **Variable syntax**: `$VAR`, `${VAR}`, arithmetic `$((expr))` — matches bash subset
- **Background jobs**: `cmd &` spawns tokio tasks, job table tracks them

## ANTI-PATTERNS

- **Don't fork external processes** — sh9 implements commands natively via HTTP client
- **Don't add commands to parser** — commands are runtime-dispatched in `eval.rs`, not grammar-level
- **Don't test with real server** in unit tests — integration tests in `tests/` handle that

## NOTES

- `eval.rs` (3152 lines) is the largest file in the project. Built-in command dispatch is a large match block
- 68 integration test scripts test end-to-end shell behavior including pipelines, variables, control flow
- Shell connects to `FS9_SERVER_URL` (default `http://localhost:9999`)
- `completer.rs` provides readline tab-completion for paths and built-in command names
