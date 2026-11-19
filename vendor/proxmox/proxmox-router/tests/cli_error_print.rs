//! Verify that command-line parse failures print a usage error to stderr.
//!
//! The parser prints straight to the process stderr, so each case re-executes this test binary:
//! the child (selected via an environment variable) builds a small nested CLI and drives it like
//! a caller of `CommandLine::try_run` does, exiting non-zero without printing anything itself on
//! error. Everything on the child's stderr therefore comes from the CLI parser, which lets the
//! parent assert that parse failures are actually reported to the user (and only once).

use std::process::Command;

use anyhow::Error;
use serde::Deserialize;
use serde_json::Value;

use proxmox_router::cli::{
    CliCommand, CliCommandMap, CommandLine, CommandLineInterface, GlobalOptions,
};
use proxmox_router::{ApiHandler, ApiMethod, RpcEnvironment};
use proxmox_schema::{ApiType, ObjectSchema, Schema, StringSchema};

const CASE_ENV: &str = "PROXMOX_ROUTER_TEST_CLI_ERROR_PRINT_CASE";

fn dummy_method(_: Value, _: &ApiMethod, _: &mut dyn RpcEnvironment) -> Result<Value, Error> {
    Ok(Value::Null)
}

const API_METHOD_CREATE: ApiMethod = ApiMethod::new(
    &ApiHandler::Sync(&dummy_method),
    &ObjectSchema::new(
        "Create something.",
        &[
            ("id", false, &StringSchema::new("Item name.").schema()),
            ("note", true, &StringSchema::new("Optional note.").schema()),
        ],
    ),
);

#[derive(Deserialize)]
#[allow(dead_code)]
struct GlobalArgs {
    config: Option<String>,
}

impl ApiType for GlobalArgs {
    const API_SCHEMA: Schema = ObjectSchema::new(
        "Global args.",
        &[(
            "config",
            true,
            &StringSchema::new("Path to a config file.").schema(),
        )],
    )
    .schema();
}

/// Run the CLI with `args` and exit like a real `try_run` caller: silently, non-zero on error.
fn child(args: &[&str]) -> ! {
    let sub = CliCommandMap::new().insert("create", CliCommand::new(&API_METHOD_CREATE));
    let cmd_def = CliCommandMap::new()
        .global_option(GlobalOptions::of::<GlobalArgs>())
        .insert("item", CommandLineInterface::Nested(sub))
        .build();

    let args = std::iter::once("clitest".to_string()).chain(args.iter().map(ToString::to_string));
    let result = CommandLine::new(cmd_def).try_run(args, |_env| Ok(()));
    std::process::exit(if result.is_err() { 1 } else { 0 });
}

struct CaseResult {
    success: bool,
    stderr: String,
}

fn run_case(case: &str) -> CaseResult {
    let exe = std::env::current_exe().expect("cannot determine current executable");
    let output = Command::new(exe)
        .env(CASE_ENV, case)
        .output()
        .expect("re-executing the test binary failed");
    CaseResult {
        success: output.status.success(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn assert_prints_error(case: &str, expected: &[&str]) {
    let res = run_case(case);
    assert!(
        !res.success,
        "case {case}: expected non-zero exit\nstderr:\n{}",
        res.stderr
    );
    for needle in expected {
        assert!(
            res.stderr.contains(needle),
            "case {case}: missing {needle:?} on stderr\nstderr:\n{}",
            res.stderr
        );
    }
    assert_eq!(
        res.stderr.matches("Error:").count(),
        1,
        "case {case}: error must be printed exactly once\nstderr:\n{}",
        res.stderr
    );
}

fn main() {
    if let Ok(case) = std::env::var(CASE_ENV) {
        match case.as_str() {
            // a per-command option placed before the subcommand
            "misplaced-option" => child(&["--note", "x", "item", "create", "--id", "y"]),
            // a typo'd global option
            "unknown-global" => child(&["--confg", "x", "item", "create", "--id", "y"]),
            // a global option missing its value, ending up at the leaf-command level
            "missing-value" => child(&["item", "create", "--id", "y", "--config"]),
            // pre-existing print behavior, guarded so the exactly-once assertion stays honest
            "unknown-command" => child(&["frobnicate"]),
            "ok" => child(&["--config", "x", "item", "create", "--id", "y"]),
            other => panic!("unknown test case {other:?}"),
        }
    }

    assert_prints_error("misplaced-option", &["unknown option", "Usage:"]);
    assert_prints_error("unknown-global", &["unknown option", "Usage:"]);
    // the leaf-level print must include the inherited global options in its usage text
    assert_prints_error(
        "missing-value",
        &[
            "missing parameter value",
            "Usage:",
            "Inherited group parameters:",
        ],
    );
    assert_prints_error("unknown-command", &["no such command", "Usage:"]);

    let ok = run_case("ok");
    assert!(
        ok.success,
        "case ok: expected success\nstderr:\n{}",
        ok.stderr
    );
    assert!(
        ok.stderr.is_empty(),
        "case ok: expected empty stderr\nstderr:\n{}",
        ok.stderr
    );

    println!("all cli-error-print cases passed");
}
