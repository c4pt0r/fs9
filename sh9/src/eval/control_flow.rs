use crate::ast::*;
use crate::error::Sh9Result;
use crate::shell::Shell;
use super::ExecContext;
use super::utils::match_glob_pattern;

impl Shell {
    pub(crate) async fn execute_test(&self, args: &[String], _ctx: &mut ExecContext) -> Sh9Result<i32> {
        let args: Vec<&str> = args.iter()
            .map(|s| s.as_str())
            .filter(|s| *s != "]")
            .collect();

        if args.is_empty() {
            return Ok(1);
        }

        // Handle ! negation
        let (negate, args) = if args.first() == Some(&"!") {
            (true, &args[1..])
        } else {
            (false, args.as_slice())
        };

        if args.is_empty() {
            return Ok(if negate { 0 } else { 1 });
        }

        let result = match args {
            [s1, "=", s2] | [s1, "==", s2] => s1 == s2,
            [s1, "!=", s2] => s1 != s2,
            [n1, "-eq", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a == b
            }
            [n1, "-ne", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a != b
            }
            [n1, "-lt", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a < b
            }
            [n1, "-le", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a <= b
            }
            [n1, "-gt", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a > b
            }
            [n1, "-ge", n2] => {
                let a: i64 = n1.parse().unwrap_or(0);
                let b: i64 = n2.parse().unwrap_or(0);
                a >= b
            }
            ["-n", s] => !s.is_empty(),
            ["-z", s] => s.is_empty(),
            ["-e", path] | ["-f", path] | ["-d", path] | ["-s", path] => {
                let op = args[0];
                let full_path = self.resolve_path(path);
                let router = self.router();
                match router.stat(&full_path).await {
                    Ok(info) => match op {
                        "-e" => true,
                        "-f" => !info.is_dir,
                        "-d" => info.is_dir,
                        "-s" => info.size > 0,
                        _ => false,
                    },
                    Err(_) => false,
                }
            }
            [s] => !s.is_empty(),
            _ => false,
        };

        let result = if negate { !result } else { result };
        Ok(if result { 0 } else { 1 })
    }

    pub(crate) async fn execute_if(&mut self, if_stmt: &IfStatement, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let cond_result = self.execute_pipeline(&if_stmt.condition, ctx).await?;
        
        if cond_result == 0 {
            let mut result = 0;
            for stmt in &if_stmt.then_body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
            Ok(result)
        } else {
            for elif in &if_stmt.elif_clauses {
                let elif_result = self.execute_pipeline(&elif.condition, ctx).await?;
                if elif_result == 0 {
                    let mut result = 0;
                    for stmt in &elif.body {
                        result = self.execute_statement_boxed(stmt, ctx).await?;
                        if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                            return Ok(result);
                        }
                    }
                    return Ok(result);
                }
            }
            
            if let Some(else_body) = &if_stmt.else_body {
                let mut result = 0;
                for stmt in else_body {
                    result = self.execute_statement_boxed(stmt, ctx).await?;
                    if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                        return Ok(result);
                    }
                }
                Ok(result)
            } else {
                Ok(0)
            }
        }
    }

    pub(crate) async fn execute_for(&mut self, for_loop: &ForLoop, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut result = 0;
        
        let mut all_items = Vec::new();
        for item in &for_loop.items {
            let expanded = self.expand_word(item, ctx).await?;
            let glob_expanded = self.expand_glob(&expanded).await;
            for glob_item in glob_expanded {
                for part in glob_item.split_whitespace() {
                    all_items.push(part.to_string());
                }
            }
        }
        
        for value in all_items {
            self.set_var(&for_loop.variable, &value);
            
            for stmt in &for_loop.body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                
                if ctx.should_break {
                    ctx.should_break = false;
                    return Ok(result);
                }
                if ctx.should_continue {
                    ctx.should_continue = false;
                    break;
                }
                if ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
        }
        
        Ok(result)
    }

    pub(crate) async fn execute_while(&mut self, while_loop: &WhileLoop, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut result = 0;
        
        loop {
            let cond_result = self.execute_pipeline(&while_loop.condition, ctx).await?;
            if cond_result != 0 {
                break;
            }
            
            for stmt in &while_loop.body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                
                if ctx.should_break {
                    ctx.should_break = false;
                    return Ok(result);
                }
                if ctx.should_continue {
                    ctx.should_continue = false;
                    break;
                }
                if ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
        }
        
        Ok(result)
    }
    
    pub(crate) async fn execute_until(&mut self, until_loop: &crate::ast::UntilLoop, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let mut result = 0;
        
        loop {
            let cond_result = self.execute_pipeline(&until_loop.condition, ctx).await?;
            if cond_result == 0 {
                break;
            }
            
            for stmt in &until_loop.body {
                result = self.execute_statement_boxed(stmt, ctx).await?;
                
                if ctx.should_break {
                    ctx.should_break = false;
                    return Ok(result);
                }
                if ctx.should_continue {
                    ctx.should_continue = false;
                    break;
                }
                if ctx.return_value.is_some() {
                    return Ok(result);
                }
            }
        }
        
        Ok(result)
    }

    pub(crate) async fn execute_case(&mut self, case_stmt: &CaseStatement, ctx: &mut ExecContext) -> Sh9Result<i32> {
        let word_value = self.expand_word(&case_stmt.word, ctx).await?;
        
        for arm in &case_stmt.arms {
            for pattern in &arm.patterns {
                let pattern_value = self.expand_word(pattern, ctx).await?;
                if match_glob_pattern(&pattern_value, &word_value) {
                    let mut last = 0;
                    for stmt in &arm.body {
                        last = self.execute_statement_boxed(stmt, ctx).await?;
                        if ctx.should_break || ctx.should_continue || ctx.return_value.is_some() {
                            return Ok(last);
                        }
                    }
                    return Ok(last);
                }
            }
        }
        
        Ok(0)
    }
}
