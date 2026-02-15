//! Parser for sh9script
//!
//! Parses a token stream into an AST.

use crate::ast::*;
use crate::lexer::Token;
use chumsky::prelude::*;

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

/// Parse a function definition
fn function_def(
    stmt: impl Parser<Token, Statement, Error = Simple<Token>> + Clone,
) -> impl Parser<Token, Statement, Error = Simple<Token>> + Clone {
    // name() { body } or function name { body }
    let paren_style = word()
        .then_ignore(just(Token::LeftParen))
        .then_ignore(just(Token::RightParen))
        .then_ignore(just(Token::LeftBrace))
        .then(stmt.clone().repeated())
        .then_ignore(just(Token::RightBrace))
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
        .then_ignore(just(Token::LeftBrace))
        .then(stmt.repeated())
        .then_ignore(just(Token::RightBrace))
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
    let compound_for = for_statement(stmt.clone()).map(PipelineElement::Compound);
    let compound_if = if_statement(stmt).map(PipelineElement::Compound);
    let simple = command().map(PipelineElement::Simple);
    let element = choice((compound_while, compound_for, compound_if, simple));

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

    choice((
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
                .map(|(is_sq, s)| {
                    if is_sq {
                        WordPart::SingleQuoted(s)
                    } else {
                        WordPart::Literal(s)
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
            Token::Word(s) => Ok(s),
            _ => Err(Simple::expected_input_found(span, None, Some(tok))),
        }))
        .then_ignore(just(Token::RightBrace))
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
            segments
                .into_iter()
                .map(|(is_sq, s)| {
                    if is_sq {
                        format!("'{}'", s)
                    } else {
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
            WordPart::Literal(s) | WordPart::SingleQuoted(s) => s.clone(),
            WordPart::Variable(s) => format!("${}", s),
            WordPart::BracedVariable(s) => format!("${{{}}}", s),
            WordPart::Arithmetic(s) => format!("$(({})", s),
            WordPart::CommandSub(s) => format!("$({})", s),
        })
        .collect()
}

/// Parse input string directly to AST
pub fn parse(input: &str) -> Result<Script, Vec<Simple<Token>>> {
    use crate::lexer::lexer;

    // First, lex the input
    let tokens = lexer().parse(input).map_err(|errs| {
        errs.into_iter()
            .map(|e| Simple::custom(0..0, e.to_string()))
            .collect::<Vec<_>>()
    })?;

    // Then parse the tokens
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
}
