use super::ExecContext;
use crate::error::{Sh9Error, Sh9Result};
use crate::shell::Shell;

impl Shell {
    pub(crate) fn evaluate_arithmetic(&self, expr: &str, ctx: &ExecContext) -> Sh9Result<i64> {
        let expr = expr.trim();
        let expanded = self.expand_arithmetic_vars(expr, ctx)?;
        self.eval_arithmetic_expr(&expanded)
    }

    fn expand_arithmetic_vars(&self, expr: &str, ctx: &ExecContext) -> Sh9Result<String> {
        let mut result = String::new();
        let mut chars = expr.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '$' {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = self.get_variable_value(&name, ctx);
                result.push_str(&value);
            } else if c.is_alphabetic() || c == '_' {
                let mut name = String::from(c);
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = self.get_variable_value(&name, ctx);
                if !value.is_empty() {
                    result.push_str(&value);
                } else {
                    result.push('0');
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    fn eval_arithmetic_expr(&self, expr: &str) -> Sh9Result<i64> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Ok(0);
        }
        self.parse_arithmetic_expr(&mut expr.chars().peekable())
    }

    fn parse_arithmetic_expr(
        &self,
        chars: &mut std::iter::Peekable<std::str::Chars>,
    ) -> Sh9Result<i64> {
        self.parse_additive(chars)
    }

    fn parse_additive(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        let mut left = self.parse_multiplicative(chars)?;

        loop {
            self.skip_whitespace(chars);
            match chars.peek() {
                Some('+') => {
                    chars.next();
                    let right = self.parse_multiplicative(chars)?;
                    left += right;
                }
                Some('-') => {
                    chars.next();
                    let right = self.parse_multiplicative(chars)?;
                    left -= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    fn parse_multiplicative(
        &self,
        chars: &mut std::iter::Peekable<std::str::Chars>,
    ) -> Sh9Result<i64> {
        let mut left = self.parse_unary(chars)?;

        loop {
            self.skip_whitespace(chars);
            match chars.peek() {
                Some('*') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    left *= right;
                }
                Some('/') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    if right == 0 {
                        return Err(Sh9Error::Runtime("Division by zero".to_string()));
                    }
                    left /= right;
                }
                Some('%') => {
                    chars.next();
                    let right = self.parse_unary(chars)?;
                    if right == 0 {
                        return Err(Sh9Error::Runtime("Division by zero".to_string()));
                    }
                    left %= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    fn parse_unary(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        self.skip_whitespace(chars);

        match chars.peek() {
            Some('-') => {
                chars.next();
                Ok(-self.parse_primary(chars)?)
            }
            Some('+') => {
                chars.next();
                self.parse_primary(chars)
            }
            _ => self.parse_primary(chars),
        }
    }

    fn parse_primary(&self, chars: &mut std::iter::Peekable<std::str::Chars>) -> Sh9Result<i64> {
        self.skip_whitespace(chars);

        match chars.peek() {
            Some('(') => {
                chars.next();
                let value = self.parse_arithmetic_expr(chars)?;
                self.skip_whitespace(chars);
                if chars.next() != Some(')') {
                    return Err(Sh9Error::Runtime("Expected ')'".to_string()));
                }
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() => {
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                num.parse::<i64>()
                    .map_err(|_| Sh9Error::Runtime(format!("Invalid number: {}", num)))
            }
            Some(_) | None => Ok(0),
        }
    }

    fn skip_whitespace(&self, chars: &mut std::iter::Peekable<std::str::Chars>) {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
    }
}
