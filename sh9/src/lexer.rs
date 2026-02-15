//! Lexer for sh9script
//!
//! Tokenizes shell input into a stream of tokens.

use chumsky::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Word(String),
    SingleQuoted(String),
    Number(String),

    // Operators
    Pipe,      // |
    Semicolon, // ;
    Newline,   // \n
    Ampersand, // &
    AndAnd,    // &&
    OrOr,      // ||

    // Redirections
    RedirectOut,       // >
    RedirectAppend,    // >>
    RedirectIn,        // <
    RedirectErr,       // 2>
    RedirectErrAppend, // 2>>
    RedirectBoth,      // &>

    // Brackets
    LeftParen,        // (
    RightParen,       // )
    LeftBrace,        // {
    RightBrace,       // }
    LeftBracket,      // [
    RightBracket,     // ]
    DoubleBracket,    // [[
    DoubleBracketEnd, // ]]

    // Assignment
    Equals, // =

    // Keywords
    If,
    Then,
    Elif,
    Else,
    Fi,
    For,
    In,
    Do,
    Done,
    While,
    Until,
    Case,
    Esac,
    Function,
    Return,
    Break,
    Continue,
    True,
    False,

    // Special
    Dollar,            // $
    DollarParen,       // $(
    DollarDoubleParen, // $((
    DollarBrace,       // ${
    Backtick,          // `

    // Compound word: adjacent bare/quoted segments merged
    // Vec<(is_single_quoted, content)>
    CompoundWord(Vec<(bool, String)>),
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Word(s) | Token::SingleQuoted(s) => write!(f, "{}", s),
            Token::Number(n) => write!(f, "{}", n),
            Token::Pipe => write!(f, "|"),
            Token::Semicolon => write!(f, ";"),
            Token::Newline => write!(f, "\\n"),
            Token::Ampersand => write!(f, "&"),
            Token::AndAnd => write!(f, "&&"),
            Token::OrOr => write!(f, "||"),
            Token::RedirectOut => write!(f, ">"),
            Token::RedirectAppend => write!(f, ">>"),
            Token::RedirectIn => write!(f, "<"),
            Token::RedirectErr => write!(f, "2>"),
            Token::RedirectErrAppend => write!(f, "2>>"),
            Token::RedirectBoth => write!(f, "&>"),
            Token::LeftParen => write!(f, "("),
            Token::RightParen => write!(f, ")"),
            Token::LeftBrace => write!(f, "{{"),
            Token::RightBrace => write!(f, "}}"),
            Token::LeftBracket => write!(f, "["),
            Token::RightBracket => write!(f, "]"),
            Token::DoubleBracket => write!(f, "[["),
            Token::DoubleBracketEnd => write!(f, "]]"),
            Token::Equals => write!(f, "="),
            Token::If => write!(f, "if"),
            Token::Then => write!(f, "then"),
            Token::Elif => write!(f, "elif"),
            Token::Else => write!(f, "else"),
            Token::Fi => write!(f, "fi"),
            Token::For => write!(f, "for"),
            Token::In => write!(f, "in"),
            Token::Do => write!(f, "do"),
            Token::Done => write!(f, "done"),
            Token::While => write!(f, "while"),
            Token::Until => write!(f, "until"),
            Token::Case => write!(f, "case"),
            Token::Esac => write!(f, "esac"),
            Token::Function => write!(f, "function"),
            Token::Return => write!(f, "return"),
            Token::Break => write!(f, "break"),
            Token::Continue => write!(f, "continue"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Dollar => write!(f, "$"),
            Token::DollarParen => write!(f, "$("),
            Token::DollarDoubleParen => write!(f, "$(("),
            Token::DollarBrace => write!(f, "${{"),
            Token::Backtick => write!(f, "`"),
            Token::CompoundWord(segments) => {
                for (is_sq, s) in segments {
                    if *is_sq {
                        write!(f, "'{}'", s)?;
                    } else {
                        write!(f, "{}", s)?;
                    }
                }
                Ok(())
            }
        }
    }
}

pub fn lexer() -> impl Parser<char, Vec<Token>, Error = Simple<char>> {
    let hash_comment = just('#').then(filter(|c| *c != '\n').repeated()).ignored();
    let slash_comment = just("//").then(filter(|c| *c != '\n').repeated()).ignored();
    let comment = hash_comment.or(slash_comment);

    // Whitespace (not including newlines)
    let ws = filter(|c: &char| *c == ' ' || *c == '\t').repeated();

    // Word segments: bare chars, single-quoted, double-quoted, escaped chars
    // Adjacent segments (no whitespace between) form a compound word.
    // (bool, String): true = single-quoted (no expansion), false = normal (expansion happens)
    let sq_seg = just('\'')
        .ignore_then(filter(|c| *c != '\'').repeated())
        .then_ignore(just('\''))
        .collect::<String>()
        .map(|s| (true, s));

    let dq_seg = just('"')
        .ignore_then(
            just('\\')
                .then(any())
                .map(|(_b, c): (char, char)| match c {
                    // POSIX: these escapes are interpreted inside double quotes
                    '"' => "\"".to_string(),
                    '\\' => "\\".to_string(),
                    '$' => "$".to_string(),
                    '`' => "`".to_string(),
                    '\n' => String::new(), // line continuation
                    // All other \X sequences are literal (backslash preserved)
                    _ => format!("\\{}", c),
                })
                .or(filter(|c: &char| *c != '"' && *c != '\\').map(|c: char| c.to_string()))
                .repeated(),
        )
        .then_ignore(just('"'))
        .map(|parts: Vec<String>| (false, parts.concat()));

    // Keywords
    let keyword = choice((
        text::keyword("if").to(Token::If),
        text::keyword("then").to(Token::Then),
        text::keyword("elif").to(Token::Elif),
        text::keyword("else").to(Token::Else),
        text::keyword("fi").to(Token::Fi),
        text::keyword("for").to(Token::For),
        text::keyword("in").to(Token::In),
        text::keyword("do").to(Token::Do),
        text::keyword("done").to(Token::Done),
        text::keyword("while").to(Token::While),
        text::keyword("until").to(Token::Until),
        text::keyword("case").to(Token::Case),
        text::keyword("esac").to(Token::Esac),
        text::keyword("function").to(Token::Function),
        text::keyword("return").to(Token::Return),
        text::keyword("break").to(Token::Break),
        text::keyword("continue").to(Token::Continue),
        text::keyword("true").to(Token::True),
        text::keyword("false").to(Token::False),
    ));

    // Multi-character operators (must come before single-char versions)
    let multi_op = choice((
        just("$((").to(Token::DollarDoubleParen),
        just("$(").to(Token::DollarParen),
        just("${").to(Token::DollarBrace),
        just("&&").to(Token::AndAnd),
        just("||").to(Token::OrOr),
        just(">>").to(Token::RedirectAppend),
        just("2>>").to(Token::RedirectErrAppend),
        just("2>").to(Token::RedirectErr),
        just("&>").to(Token::RedirectBoth),
        just("[[").to(Token::DoubleBracket),
        just("]]").to(Token::DoubleBracketEnd),
    ));

    let single_op = choice((
        just('|').to(Token::Pipe),
        just(';').to(Token::Semicolon),
        just('&').to(Token::Ampersand),
        just('>').to(Token::RedirectOut),
        just('<').to(Token::RedirectIn),
        just('(').to(Token::LeftParen),
        just(')').to(Token::RightParen),
        just('{').to(Token::LeftBrace),
        just('}').to(Token::RightBrace),
        just('[').to(Token::Word("[".to_string())),
        just(']').to(Token::Word("]".to_string())),
        just("==").to(Token::Word("==".to_string())),
        just("!=").to(Token::Word("!=".to_string())),
        just('=').to(Token::Equals),
        just('$').to(Token::Dollar),
        just('\n').to(Token::Newline),
        just('`').to(Token::Backtick),
    ));

    let word_char = filter(|c: &char| {
        !c.is_whitespace()
            && !matches!(
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

    // Backslash-escape outside quotes: \X → literal X (POSIX)
    // \<newline> → line continuation (empty)
    let escaped_char = just('\\')
        .ignore_then(any())
        .map(|c: char| if c == '\n' { String::new() } else { c.to_string() });

    let bare_seg = escaped_char
        .or(word_char.map(|c: char| c.to_string()))
        .repeated()
        .at_least(1)
        .map(|parts| (false, parts.concat()));

    // Compound word: one or more adjacent segments (bare, single-quoted, double-quoted)
    // with no whitespace between them. Produces the appropriate token type.
    let compound_word = choice((bare_seg, sq_seg, dq_seg))
        .repeated()
        .at_least(1)
        .map(|segments: Vec<(bool, String)>| {
            if segments.len() == 1 {
                let (is_sq, s) = segments.into_iter().next().unwrap();
                if is_sq {
                    Token::SingleQuoted(s)
                } else {
                    Token::Word(s)
                }
            } else {
                Token::CompoundWord(segments)
            }
        });

    // Token: operators, keywords, then compound words (bare+quoted merged)
    let token = choice((
        multi_op,
        single_op,
        keyword,
        compound_word,
    ));

    // Skip comments and whitespace between tokens
    token
        .padded_by(comment.repeated())
        .padded_by(ws)
        .repeated()
        .then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<Token> {
        lexer().parse(input).unwrap()
    }

    #[test]
    fn test_simple_command() {
        let tokens = lex("echo hello");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello".to_string()),
            ]
        );
    }

    #[test]
    fn test_quoted_string() {
        let tokens = lex("echo \"hello world\"");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello world".to_string()),
            ]
        );
    }

    #[test]
    fn test_single_quoted() {
        let tokens = lex("echo 'hello $world'");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::SingleQuoted("hello $world".to_string()),
            ]
        );
    }

    #[test]
    fn test_pipeline() {
        let tokens = lex("ls | grep foo");
        assert_eq!(
            tokens,
            vec![
                Token::Word("ls".to_string()),
                Token::Pipe,
                Token::Word("grep".to_string()),
                Token::Word("foo".to_string()),
            ]
        );
    }

    #[test]
    fn test_redirection() {
        let tokens = lex("echo hello > file.txt");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello".to_string()),
                Token::RedirectOut,
                Token::Word("file.txt".to_string()),
            ]
        );
    }

    #[test]
    fn test_variable() {
        let tokens = lex("echo $foo");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Dollar,
                Token::Word("foo".to_string()),
            ]
        );
    }

    #[test]
    fn test_assignment() {
        let tokens = lex("x=5");
        assert_eq!(
            tokens,
            vec![
                Token::Word("x".to_string()),
                Token::Equals,
                Token::Word("5".to_string()),
            ]
        );
    }

    #[test]
    fn test_if_statement() {
        let tokens = lex("if true; then echo yes; fi");
        assert_eq!(
            tokens,
            vec![
                Token::If,
                Token::True,
                Token::Semicolon,
                Token::Then,
                Token::Word("echo".to_string()),
                Token::Word("yes".to_string()),
                Token::Semicolon,
                Token::Fi,
            ]
        );
    }

    #[test]
    fn test_comment() {
        let tokens = lex("echo hello # this is a comment\necho world");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello".to_string()),
                Token::Newline,
                Token::Word("echo".to_string()),
                Token::Word("world".to_string()),
            ]
        );
    }

    #[test]
    fn test_slash_comment() {
        let tokens = lex("echo hello // this is a comment\necho world");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".to_string()),
                Token::Word("hello".to_string()),
                Token::Newline,
                Token::Word("echo".to_string()),
                Token::Word("world".to_string()),
            ]
        );
    }

    #[test]
    fn test_backtick() {
        let tokens = lex("`ls`");
        assert_eq!(
            tokens,
            vec![
                Token::Backtick,
                Token::Word("ls".to_string()),
                Token::Backtick,
            ]
        );
    }

    #[test]
    fn test_arithmetic() {
        let tokens = lex("$((1 + 2))");
        assert_eq!(
            tokens,
            vec![
                Token::DollarDoubleParen,
                Token::Word("1".to_string()),
                Token::Word("+".to_string()),
                Token::Word("2".to_string()),
                Token::RightParen,
                Token::RightParen,
            ]
        );
    }

    #[test]
    fn test_braced_variable() {
        let tokens = lex("${foo}");
        assert_eq!(
            tokens,
            vec![
                Token::DollarBrace,
                Token::Word("foo".to_string()),
                Token::RightBrace,
            ]
        );
    }

    #[test]
    fn test_background() {
        let tokens = lex("sleep 5 &");
        assert_eq!(
            tokens,
            vec![
                Token::Word("sleep".to_string()),
                Token::Word("5".to_string()),
                Token::Ampersand,
            ]
        );
    }
}
