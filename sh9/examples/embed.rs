//! Embed sh9 as a scripting engine in your Rust application.
//!
//! Run:  cargo run -p sh9 --example embed

use sh9::{Sh9Result, Shell, ShellBuilder};

fn print_capture(label: &str, out: &sh9::CapturedOutput) {
    println!("\n== {label} ==");
    println!("exit: {}", out.exit_code);

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.is_empty() {
        println!("stdout: <empty>");
    } else {
        println!("stdout:\n{}", stdout);
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.is_empty() {
        println!("stderr: <empty>");
    } else {
        println!("stderr:\n{}", stderr);
    }
}

#[tokio::main]
async fn main() -> Sh9Result<()> {
    let mut shell = ShellBuilder::new("http://localhost:9999")
        .env("APP_NAME", "embed-demo")
        .builtin("set_from_builder", |args: &[String], shell: &mut Shell| {
            let value = args.first().map_or("unset", String::as_str);
            shell.set_var("FROM_BUILDER", value);
            Ok(0)
        })
        .build();

    println!("sh9 embedded demo");
    println!("APP_NAME from builder env: {:?}", shell.get_var("APP_NAME"));

    let out = shell.execute_capture("set_from_builder configured").await?;
    print_capture("builder builtin", &out);
    println!(
        "FROM_BUILDER after command: {:?}",
        shell.get_var("FROM_BUILDER")
    );

    shell.set_var("EXPLICIT", "set_via_set_var");
    let out = shell.execute_capture("echo $EXPLICIT").await?;
    print_capture("set_var/get_var", &out);

    let out = shell.execute_capture("x=42; echo $((x * 2))").await?;
    print_capture("variables + arithmetic", &out);

    let out = shell
        .execute_capture("for i in 1 2 3; do echo \"item $i\"; done")
        .await?;
    print_capture("control flow (for loop)", &out);

    shell
        .execute("greet() { echo \"hello from function\"; }")
        .await?;
    let out = shell.execute_capture("greet").await?;
    print_capture("function definition + call", &out);

    println!("\n== pipeline ==");
    let pipeline_exit = shell
        .execute("echo \"apple\nbanana\napricot\" | grep \"ap\" | wc -l")
        .await?;
    println!("exit: {pipeline_exit}");

    shell.register_builtin("set_runtime", |args: &[String], shell: &mut Shell| {
        let value = args.first().map_or("unset", String::as_str);
        shell.set_var("FROM_RUNTIME", value);
        Ok(0)
    });
    let out = shell.execute_capture("set_runtime dynamic").await?;
    print_capture("runtime register_builtin", &out);
    println!(
        "FROM_RUNTIME after command: {:?}",
        shell.get_var("FROM_RUNTIME")
    );

    let out = shell.execute_capture("true").await?;
    print_capture("exit code from true", &out);

    let out = shell.execute_capture("false").await?;
    print_capture("exit code from false", &out);

    let out = shell.execute_capture("cat /nonexistent").await?;
    print_capture("stderr capture (no server)", &out);

    Ok(())
}
