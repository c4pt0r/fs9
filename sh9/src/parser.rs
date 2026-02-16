//! Parser for sh9script
//!
//! Parses a token stream into an AST.

use crate::ast::*;
use crate::lexer::Token;
use chumsky::prelude::*;
use std::collections::VecDeque;

/// Parse a token stream into a Script AST
pub fn parser() -> impl Parser<Token, Script, Error = Simple<Token>> {
    statement()
        .repeated()
        .map(|statements| Script { statements })
        .then_ignore(end())
}

/// Parse a single statement
fn statement() -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    recursive(|stmt| {
        // Statement separators (semicolon or newline)
        let sep = filter(|t| matches!(t, Token::Semicolon | Token::Newline)).repeated();

        // Empty statement (just separators)
        let empty = sep.at_least(1).to(Statement::Empty);

        // Assignment: name=value
        let assignment = word()
            .then_ignore(just(Token::Equals))
            .then(word().or_not())
            .map(|(name, value)| {
                let name_str = word_to_string(&name);
                Statement::Assignment(Assignment {
                    name: name_str,
                    value: value.unwrap_or_else(Word::empty),
                })
            });

        // Break statement
        let break_stmt = just(Token::Break).to(Statement::Break);

        // Continue statement
        let continue_stmt = just(Token::Continue).to(Statement::Continue);

        // Return statement
        let return_stmt = just(Token::Return)
            .ignore_then(word().or_not())
            .map(Statement::Return);

        // Function definition
        let func_def = function_def(stmt.clone());

        // Command list: pipeline { && | || pipeline }*
        // pipeline(stmt) handles both simple commands AND compound commands (while/for/if)
        let command_list_stmt = pipeline(stmt.clone())
            .then(
                choice((
                    just(Token::AndAnd).to(ListOp::And),
                    just(Token::OrOr).to(ListOp::Or),
                ))
                .then(pipeline(stmt))
                .repeated(),
            )
            .map(|(first, rest)| {
                if rest.is_empty() {
                    Statement::Pipeline(first)
                } else {
                    Statement::CommandList { first, rest }
                }
            });

        choice((
            empty,
            break_stmt,
            continue_stmt,
            return_stmt,
            func_def,
            assignment,
            command_list_stmt,
        ))
        .padded_by(sep)
    })
}

/// Parse an if statement
fn if_statement(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // if condition; then body; elif condition; then body; else body; fi
    just(Token::If)
        .ignore_then(simple_pipeline())
        .then_ignore(just(Token::Semicolon).or_not())
        .then_ignore(just(Token::Then))
        .then(stmt.clone().repeated())
        .then(
            just(Token::Elif)
                .ignore_then(simple_pipeline())
                .then_ignore(just(Token::Semicolon).or_not())
                .then_ignore(just(Token::Then))
                .then(stmt.clone().repeated())
                .map(|(cond, body)| ElifClause {
                    condition: cond,
                    body,
                })
                .repeated(),
        )
        .then(just(Token::Else).ignore_then(stmt.repeated()).or_not())
        .then_ignore(just(Token::Fi))
        .map(|(((condition, then_body), elif_clauses), else_body)| {
            Statement::If(IfStatement {
                condition: Box::new(condition),
                then_body,
                elif_clauses,
                else_body,
            })
        })
}

/// Parse a for loop
fn for_statement(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // for var in items; do body; done
    just(Token::For)
        .ignore_then(word())
        .then_ignore(just(Token::In))
        .then(word().repeated())
        .then_ignore(just(Token::Semicolon).or_not())
        .then_ignore(just(Token::Do))
        .then(stmt.repeated())
        .then_ignore(just(Token::Done))
        .map(|((var, items), body)| {
            Statement::For(ForLoop {
                variable: word_to_string(&var),
                items,
                body,
            })
        })
}

/// Parse a while loop
fn while_statement(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // while condition; do body; done
    just(Token::While)
        .ignore_then(simple_pipeline())
        .then_ignore(just(Token::Semicolon).or_not())
        .then_ignore(just(Token::Do))
        .then(stmt.repeated())
        .then_ignore(just(Token::Done))
        .map(|(condition, body)| {
            Statement::While(WhileLoop {
                condition: Box::new(condition),
                body,
            })
        })
}

/// Parse an until loop
fn until_statement(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // until condition; do body; done
    just(Token::Until)
        .ignore_then(simple_pipeline())
        .then_ignore(just(Token::Semicolon).or_not())
        .then_ignore(just(Token::Do))
        .then(stmt.repeated())
        .then_ignore(just(Token::Done))
        .map(|(condition, body)| {
            Statement::Until(UntilLoop {
                condition: Box::new(condition),
                body,
            })
        })
}

/// Parse a case statement
fn case_statement(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // case word in pattern|pattern) body;; ... esac
    let newline_sep = filter(|t| matches!(t, Token::Newline)).repeated();

    let case_arm = word()
        .separated_by(just(Token::Pipe))
        .at_least(1)
        .then_ignore(just(Token::RightParen))
        .then(stmt.clone().repeated())
        .then_ignore(just(Token::Semicolon))
        .then_ignore(just(Token::Semicolon))
        .then_ignore(newline_sep)
        .map(|(patterns, body)| CaseArm { patterns, body });

    just(Token::Case)
        .ignore_then(word())
        .then_ignore(just(Token::In))
        .then_ignore(newline_sep)
        .then(case_arm.repeated())
        .then_ignore(just(Token::Esac))
        .map(|(word, arms)| Statement::Case(CaseStatement { word, arms }))
}

/// Parse a function definition
fn function_def(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // name() { body } or function name { body }
    let left_brace = just(Token::Word("{".to_string()));
    let right_brace = just(Token::Word("}".to_string()));

    let paren_style = word()
        .then_ignore(just(Token::LeftParen))
        .then_ignore(just(Token::RightParen))
        .then_ignore(left_brace.clone())
        .then(stmt.clone().repeated())
        .then_ignore(right_brace.clone())
        .map(|(name, body)| {
            Statement::FunctionDef(FunctionDef {
                name: word_to_string(&name),
                body,
            })
        });

    let function_keyword = just(Token::Function)
        .ignore_then(word())
        .then_ignore(
            just(Token::LeftParen)
                .then(just(Token::RightParen))
                .or_not(),
        )
        .then_ignore(left_brace)
        .then(stmt.repeated())
        .then_ignore(right_brace)
        .map(|(name, body)| {
            Statement::FunctionDef(FunctionDef {
                name: word_to_string(&name),
                body,
            })
        });

    paren_style.or(function_keyword)
}

/// Parse a simple pipeline (only simple commands, used in conditions)
fn simple_pipeline() -> impl Parser<Token, Pipeline, Error = Simple<Token>> + Clone {
    command()
        .separated_by(just(Token::Pipe))
        .at_least(1)
        .then(just(Token::Ampersand).or_not())
        .map(|(commands, bg)| Pipeline {
            elements: commands.into_iter().map(PipelineElement::Simple).collect(),
            background: bg.is_some(),
        })
}

/// Parse a pipeline that can contain compound commands (while/for/if) in pipe positions
fn pipeline(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Pipeline, Error = Simple<Token>> + Clone {
    let compound_while = while_statement(stmt.clone()).map(PipelineElement::Compound);
    let compound_until = until_statement(stmt.clone()).map(PipelineElement::Compound);
    let compound_for = for_statement(stmt.clone()).map(PipelineElement::Compound);
    let compound_if = if_statement(stmt.clone()).map(PipelineElement::Compound);
    let compound_case = case_statement(stmt).map(PipelineElement::Compound);
    let simple = command().map(PipelineElement::Simple);
    let element = choice((
        compound_while,
        compound_until,
        compound_for,
        compound_if,
        compound_case,
        simple,
    ));

    element
        .separated_by(just(Token::Pipe))
        .at_least(1)
        .then(just(Token::Ampersand).or_not())
        .map(|(elements, bg)| Pipeline {
            elements,
            background: bg.is_some(),
        })
}

/// Parse a single command
fn command() -> impl Parser<Token, Command, Error = Simple<Token>> + Clone {
    // Command is: name [args...] [redirections...]
    word()
        .then(word().repeated())
        .then(redirection().repeated())
        .map(|((name, args), redirections)| Command {
            name,
            args,
            redirections,
        })
}

/// Parse a redirection
fn redirection() -> impl Parser<Token, Redirection, Error = Simple<Token>> + Clone {
    let redirect_out = just(Token::RedirectOut)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::StdoutWrite,
            target,
        });

    let redirect_append = just(Token::RedirectAppend)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::StdoutAppend,
            target,
        });

    let redirect_in = just(Token::RedirectIn)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::StdinRead,
            target,
        });

    let redirect_err = just(Token::RedirectErr)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::StderrWrite,
            target,
        });

    let redirect_err_append = just(Token::RedirectErrAppend)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::StderrAppend,
            target,
        });

    let redirect_both = just(Token::RedirectBoth)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::BothWrite,
            target,
        });

    let heredoc_redir = just(Token::HereDoc)
        .ignore_then(word())
        .ignore_then(filter_map(|span, tok| match tok {
            Token::HereDocBody(content, expand) => Ok((content, expand)),
            _ => Err(Simple::expected_input_found(span, None, Some(tok))),
        }))
        .map(|(content, expand)| Redirection {
            kind: RedirectKind::HereDoc { content, expand },
            target: Word::empty(),
        });

    let herestring_redir = just(Token::HereString)
        .ignore_then(word())
        .map(|target| Redirection {
            kind: RedirectKind::HereString,
            target,
        });

    choice((
        heredoc_redir,
        herestring_redir,
        redirect_append,     // Must come before redirect_out
        redirect_err_append, // Must come before redirect_err
        redirect_out,
        redirect_err,
        redirect_in,
        redirect_both,
    ))
}

fn word() -> impl Parser<Token, Word, Error = Simple<Token>> + Clone {
    let simple_word = filter_map(|span, tok| match tok {
        Token::Word(s) => Ok(Word {
            parts: vec![WordPart::Literal(s)],
        }),
        Token::SingleQuoted(s) => Ok(Word {
            parts: vec![WordPart::SingleQuoted(s)],
        }),
        Token::CompoundWord(segments) => Ok(Word {
            parts: segments
                .into_iter()
                .map(|(quote_type, s)| {
                    use crate::lexer::QuoteType;
                    match quote_type {
                        QuoteType::SingleQuoted => WordPart::SingleQuoted(s),
                        QuoteType::DoubleQuoted => WordPart::DoubleQuoted(s),
                        QuoteType::Bare => WordPart::Literal(s),
                    }
                })
                .collect(),
        }),
        Token::Number(n) => Ok(Word {
            parts: vec![WordPart::Literal(n)],
        }),
        Token::True => Ok(Word {
            parts: vec![WordPart::Literal("true".to_string())],
        }),
        Token::False => Ok(Word {
            parts: vec![WordPart::Literal("false".to_string())],
        }),
        Token::Equals => Ok(Word {
            parts: vec![WordPart::Literal("=".to_string())],
        }),
        _ => Err(Simple::expected_input_found(span, None, Some(tok))),
    });

    let var_ref = just(Token::Dollar).ignore_then(filter_map(|span, tok| match tok {
        Token::Word(s) => Ok(Word {
            parts: vec![WordPart::Variable(s)],
        }),
        _ => Err(Simple::expected_input_found(span, None, Some(tok))),
    }));

    let braced_var = just(Token::DollarBrace)
        .ignore_then(filter_map(|span, tok| match tok {
            Token::Word(s) => {
                if let Some(inner) = s.strip_suffix('}') {
                    Ok(inner.to_string())
                } else {
                    Err(Simple::expected_input_found(
                        span,
                        None,
                        Some(Token::Word(s)),
                    ))
                }
            }
            _ => Err(Simple::expected_input_found(span, None, Some(tok))),
        }))
        .map(|s| Word {
            parts: vec![WordPart::BracedVariable(s)],
        });

    let arithmetic = just(Token::DollarDoubleParen)
        .ignore_then(collect_until_double_paren())
        .map(|expr| Word {
            parts: vec![WordPart::Arithmetic(expr)],
        });

    let command_sub = just(Token::DollarParen)
        .ignore_then(collect_until_paren())
        .map(|cmd| Word {
            parts: vec![WordPart::CommandSub(cmd)],
        });

    let backtick_sub = just(Token::Backtick)
        .ignore_then(collect_until_backtick())
        .map(|cmd| Word {
            parts: vec![WordPart::CommandSub(cmd)],
        });

    choice((
        arithmetic,
        command_sub,
        backtick_sub,
        braced_var,
        var_ref,
        simple_word,
    ))
}

/// Convert a token to a string that can be safely re-parsed by the lexer.
/// Word tokens containing special characters are wrapped in double quotes.
fn token_to_safe_string(tok: Token) -> String {
    match tok {
        Token::Word(s) => {
            let needs_quoting = s.chars().any(|c| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '|' | '&'
                            | ';'
                            | '<'
                            | '>'
                            | '('
                            | ')'
                            | '{'
                            | '}'
                            | '$'
                            | '"'
                            | '\''
                            | '#'
                            | '='
                            | '\\'
                            | '`'
                    )
            });
            if needs_quoting {
                let mut escaped = String::with_capacity(s.len() + 2);
                escaped.push('"');
                for c in s.chars() {
                    if c == '\\' || c == '"' {
                        escaped.push('\\');
                    }
                    escaped.push(c);
                }
                escaped.push('"');
                escaped
            } else {
                s
            }
        }
        Token::SingleQuoted(s) => format!("'{}'", s),
        Token::CompoundWord(segments) => {
            use crate::lexer::QuoteType;
            segments
                .into_iter()
                .map(|(quote_type, s)| {
                    match quote_type {
                        QuoteType::SingleQuoted => format!("'{}'", s),
                        QuoteType::DoubleQuoted => {
                            let mut escaped = String::with_capacity(s.len() + 2);
                            escaped.push('"');
                            for c in s.chars() {
                                if c == '\\' || c == '"' {
                                    escaped.push('\\');
                                }
                                escaped.push(c);
                            }
                            escaped.push('"');
                            escaped
                        }
                        QuoteType::Bare => {
                            // Same quoting logic as Word
                            let needs_quoting = s.chars().any(|c| {
                                c.is_whitespace()
                                    || matches!(
                                        c,
                                        '|' | '&'
                                            | ';'
                                            | '<'
                                            | '>'
                                            | '('
                                            | ')'
                                            | '{'
                                            | '}'
                                            | '$'
                                            | '"'
                                            | '\''
                                            | '#'
                                            | '='
                                            | '\\'
                                            | '`'
                                    )
                            });
                            if needs_quoting {
                                let mut escaped = String::with_capacity(s.len() + 2);
                                escaped.push('"');
                                for c in s.chars() {
                                    if c == '\\' || c == '"' {
                                        escaped.push('\\');
                                    }
                                    escaped.push(c);
                                }
                                escaped.push('"');
                                escaped
                            } else {
                                s
                            }
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        }
        Token::Newline => ";".to_string(),
        other => other.to_string(),
    }
}

fn collect_until_double_paren() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    recursive(|inner| {
        choice((
            just(Token::LeftParen)
                .ignore_then(inner.clone())
                .then_ignore(just(Token::RightParen))
                .map(|s| format!("({})", s)),
            filter(|t| !matches!(t, Token::RightParen | Token::LeftParen))
                .map(token_to_safe_string),
        ))
        .repeated()
        .map(|parts| {
            let mut result = String::new();
            let mut prev_was_dollar = false;
            for part in parts {
                if prev_was_dollar
                    && (part
                        .chars()
                        .next()
                        .map(|c| c.is_alphanumeric() || c == '_')
                        .unwrap_or(false))
                {
                    result.push_str(&part);
                } else if !result.is_empty() && !prev_was_dollar {
                    result.push(' ');
                    result.push_str(&part);
                } else {
                    result.push_str(&part);
                }
                prev_was_dollar = part == "$";
            }
            result
        })
    })
    .then_ignore(just(Token::RightParen))
    .then_ignore(just(Token::RightParen))
}

fn collect_until_paren() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    recursive(|inner| {
        choice((
            just(Token::LeftParen)
                .ignore_then(inner.clone())
                .then_ignore(just(Token::RightParen))
                .map(|s| format!("({})", s)),
            just(Token::DollarParen)
                .ignore_then(inner.clone())
                .then_ignore(just(Token::RightParen))
                .map(|s| format!("$({})", s)),
            just(Token::DollarDoubleParen)
                .ignore_then(inner.clone())
                .then_ignore(just(Token::RightParen))
                .then_ignore(just(Token::RightParen))
                .map(|s| format!("$(({}))", s)),
            filter(|t| {
                !matches!(
                    t,
                    Token::RightParen
                        | Token::LeftParen
                        | Token::DollarParen
                        | Token::DollarDoubleParen
                )
            })
            .map(token_to_safe_string),
        ))
        .repeated()
        .map(|parts| {
            let mut result = String::new();
            let mut prev_was_dollar = false;
            for part in parts {
                if prev_was_dollar
                    && (part
                        .chars()
                        .next()
                        .map(|c| c.is_alphanumeric() || c == '_')
                        .unwrap_or(false))
                {
                    result.push_str(&part);
                } else if !result.is_empty()
                    && !prev_was_dollar
                    && !part.starts_with('$')
                    && !part.starts_with('(')
                {
                    result.push(' ');
                    result.push_str(&part);
                } else {
                    result.push_str(&part);
                }
                prev_was_dollar = part == "$";
            }
            result
        })
    })
    .then_ignore(just(Token::RightParen))
}

fn collect_until_backtick() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    filter(|t| !matches!(t, Token::Backtick))
        .repeated()
        .map(|tokens: Vec<Token>| {
            let mut result = String::new();
            let mut prev_was_dollar = false;
            for tok in tokens {
                let part = token_to_safe_string(tok);
                if prev_was_dollar
                    && (part
                        .chars()
                        .next()
                        .map(|c| c.is_alphanumeric() || c == '_')
                        .unwrap_or(false))
                {
                    result.push_str(&part);
                } else if !result.is_empty() && !prev_was_dollar && !part.starts_with('$') {
                    result.push(' ');
                    result.push_str(&part);
                } else {
                    result.push_str(&part);
                }
                prev_was_dollar = part == "$";
            }
            result
        })
        .then_ignore(just(Token::Backtick))
}

fn word_to_string(word: &Word) -> String {
    word.parts
        .iter()
        .map(|p| match p {
            WordPart::Literal(s) | WordPart::SingleQuoted(s) | WordPart::DoubleQuoted(s) => {
                s.clone()
            }
            WordPart::Variable(s) => format!("${}", s),
            WordPart::BracedVariable(s) => format!("${{{}}}", s),
            WordPart::Arithmetic(s) => format!("$(({})", s),
            WordPart::CommandSub(s) => format!("$({})", s),
        })
        .collect()
}

fn preprocess_heredocs(input: &str) -> Result<(String, Vec<(String, bool)>), String> {
    let mut source = String::new();
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let lines: Vec<&str> = input.split('\n').collect();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        source.push_str(line);
        if i + 1 < lines.len() {
            source.push('\n');
        }

        let markers = parse_heredoc_markers(line);
        i += 1;

        for (delimiter, expand) in markers {
            let mut body_lines = Vec::new();
            let mut found = false;

            while i < lines.len() {
                let current = lines[i];
                if current == delimiter {
                    found = true;
                    i += 1;
                    break;
                }
                body_lines.push(current);
                i += 1;
            }

            if !found {
                return Err(format!(
                    "unterminated heredoc: missing delimiter {}",
                    delimiter
                ));
            }

            let content = if body_lines.is_empty() {
                String::new()
            } else {
                let mut joined = body_lines.join("\n");
                joined.push('\n');
                joined
            };
            heredocs.push((content, expand));
        }
    }

    Ok((source, heredocs))
}

fn parse_heredoc_markers(line: &str) -> Vec<(String, bool)> {
    let bytes = line.as_bytes();
    let mut markers = Vec::new();
    let mut i = 0usize;

    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            if i + 2 < bytes.len() && bytes[i + 2] == b'<' {
                i += 3;
                continue;
            }

            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            if i >= bytes.len() {
                break;
            }

            if bytes[i] == b'\'' || bytes[i] == b'"' {
                let quote = bytes[i];
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                if start < i {
                    markers.push((line[start..i].to_string(), false));
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }

            let start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b';' | b'<' | b'>' | b'|' | b'&')
            {
                i += 1;
            }

            if start < i {
                markers.push((line[start..i].to_string(), true));
            }
            continue;
        }
        i += 1;
    }

    markers
}

fn inject_heredoc_tokens(
    tokens: Vec<Token>,
    heredocs: Vec<(String, bool)>,
) -> Result<Vec<Token>, String> {
    let mut pending: VecDeque<(String, bool)> = heredocs.into();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        let tok = tokens[i].clone();
        out.push(tok.clone());

        if tok == Token::HereDoc {
            i += 1;
            if i >= tokens.len() {
                return Err("heredoc missing delimiter token".to_string());
            }
            out.push(tokens[i].clone());

            if let Some((content, expand)) = pending.pop_front() {
                out.push(Token::HereDocBody(content, expand));
            } else {
                return Err("heredoc body missing for << operator".to_string());
            }
        }

        i += 1;
    }

    if !pending.is_empty() {
        return Err("extra heredoc bodies with no << operator".to_string());
    }

    Ok(out)
}

/// Parse input string directly to AST
pub fn parse(input: &str) -> Result<Script, Vec<Simple<Token>>> {
    use crate::lexer::lexer;

    let (preprocessed, heredocs) =
        preprocess_heredocs(input).map_err(|msg| vec![Simple::custom(0..0, msg)])?;

    let tokens = lexer().parse(preprocessed.as_str()).map_err(|errs| {
        errs.into_iter()
            .map(|e| Simple::custom(0..0, e.to_string()))
            .collect::<Vec<_>>()
    })?;

    let tokens =
        inject_heredoc_tokens(tokens, heredocs).map_err(|msg| vec![Simple::custom(0..0, msg)])?;

    parser().parse(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lexer;

    fn parse_tokens(input: &str) -> Result<Script, Vec<Simple<Token>>> {
        let tokens = lexer().parse(input).unwrap();
        parser().parse(tokens)
    }

    #[test]
    fn test_simple_command() {
        let script = parse_tokens("echo hello").unwrap();
        assert_eq!(script.statements.len(), 1);
        if let Statement::Pipeline(p) = &script.statements[0] {
            assert_eq!(p.elements.len(), 1);
            assert!(!p.background);
        } else {
            panic!("Expected pipeline");
        }
    }

    #[test]
    fn test_assignment() {
        let script = parse_tokens("x=5").unwrap();
        assert_eq!(script.statements.len(), 1);
        if let Statement::Assignment(a) = &script.statements[0] {
            assert_eq!(a.name, "x");
        } else {
            panic!("Expected assignment");
        }
    }

    #[test]
    fn test_pipeline() {
        let script = parse_tokens("ls | grep foo").unwrap();
        if let Statement::Pipeline(p) = &script.statements[0] {
            assert_eq!(p.elements.len(), 2);
        } else {
            panic!("Expected pipeline");
        }
    }

    #[test]
    fn test_background() {
        let script = parse_tokens("sleep 5 &").unwrap();
        if let Statement::Pipeline(p) = &script.statements[0] {
            assert!(p.background);
        } else {
            panic!("Expected pipeline");
        }
    }

    #[test]
    fn test_multiple_statements() {
        let script = parse_tokens("echo one; echo two").unwrap();
        // Filter out Empty statements
        let stmts: Vec<_> = script
            .statements
            .iter()
            .filter(|s| !matches!(s, Statement::Empty))
            .collect();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_preprocess_heredoc_collects_expected_body() {
        let (source, heredocs) = preprocess_heredocs("cat <<EOF\nhello\nworld\nEOF").unwrap();
        assert_eq!(source, "cat <<EOF\n");
        assert_eq!(heredocs, vec![("hello\nworld\n".to_string(), true)]);
    }

    #[test]
    fn test_preprocess_quoted_heredoc_disables_expansion() {
        let (source, heredocs) = preprocess_heredocs("cat <<'EOF'\n$X\nEOF").unwrap();
        assert_eq!(source, "cat <<'EOF'\n");
        assert_eq!(heredocs, vec![("$X\n".to_string(), false)]);
    }

    #[test]
    fn test_parse_heredoc_redirection_content() {
        let script = parse("cat <<EOF\nhello\nworld\nEOF").unwrap();
        let Statement::Pipeline(pipeline) = &script.statements[0] else {
            panic!("expected pipeline");
        };
        let PipelineElement::Simple(cmd) = &pipeline.elements[0] else {
            panic!("expected simple command");
        };
        assert_eq!(cmd.redirections.len(), 1);
        match &cmd.redirections[0].kind {
            RedirectKind::HereDoc { content, expand } => {
                assert_eq!(content, "hello\nworld\n");
                assert!(*expand);
            }
            other => panic!("expected heredoc, got {:?}", other),
        }
    }
}
