//! `swapcraft-cli`: voidflare without a window.
//!
//!   swapcraft-cli formats                          what converts to what
//!   swapcraft-cli convert <file> --to <format>     one conversion; --out-dir DIR, --json
//!   swapcraft-cli engines                          which engines were found
//!   swapcraft-cli mcp [--allow DIR]... [--allow-all]   MCP server on stdin/stdout, for LLMs
//!
//! Output is JSON when `--json` is given (or always for `formats` and `engines`); the exit code is 0 on
//! success and 1 on failure, with `{"error": "..."}` on stdout in JSON mode.

mod api;
mod mcp;

use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "swapcraft-cli: convert files offline, no window.

USAGE:
  swapcraft-cli formats                           what converts to what (JSON)
  swapcraft-cli convert <file> --to <format> [--out-dir DIR] [--json]
  swapcraft-cli engines                           which engines were found (JSON)
  swapcraft-cli recipes                           the recipes saved in the hub (JSON)
  swapcraft-cli recipe <name> <file|folder>... [--json]
                                                  run a saved recipe on files
  swapcraft-cli mcp [--allow DIR]... [--allow-all]
                                                  MCP server on stdin/stdout. It may only touch the folder
                                                  it was started in plus each --allow DIR (--allow-all: anywhere).
  swapcraft-cli --version
";

fn main() -> ExitCode {
    // the DWG conversion words its notes and errors from its app's catalog (otherwise the key would show)
    vf_core::i18n::init(&[include_str!("../../app-dwg-to-pdf/assets/strings.tsv")]);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("formats") => print_json(&api::formats()),
        Some("engines") => print_json(&api::engines()),
        Some("convert") => convert(&args[1..]),
        Some("recipes") => match api::recipes() {
            Ok(v) => print_json(&v),
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(1)
            }
        },
        Some("recipe") => run_recipe(&args[1..]),
        Some("mcp") => serve_mcp(&args[1..]),
        Some("--version" | "-V") => {
            println!("swapcraft-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command '{other}'\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn print_json(v: &serde_json::Value) -> ExitCode {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    ExitCode::SUCCESS
}

fn convert(args: &[String]) -> ExitCode {
    let (mut file, mut to, mut out_dir, mut json) = (None, None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => to = it.next().cloned(),
            "--out-dir" => out_dir = it.next().map(PathBuf::from),
            "--json" => json = true,
            other if file.is_none() && !other.starts_with("--") => file = Some(PathBuf::from(other)),
            other => {
                eprintln!("unexpected argument '{other}'\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(file), Some(to)) = (file, to) else {
        eprintln!("convert needs a file and --to <format>\n\n{HELP}");
        return ExitCode::from(2);
    };
    match api::convert(&file, &to, out_dir.as_deref()) {
        Ok(v) if json => print_json(&v),
        Ok(v) => {
            println!("{}", v["output"].as_str().unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) if json => {
            println!("{}", serde_json::json!({ "error": e }));
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_recipe(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let Some((name, files)) = rest.split_first().filter(|(_, f)| !f.is_empty()) else {
        eprintln!("recipe needs a recipe name and at least one file\n\n{HELP}");
        return ExitCode::from(2);
    };
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let result = api::find_recipe(name).and_then(|r| api::recipe_run(&r, &files));
    match result {
        Ok(v) if json => print_json(&v),
        Ok(v) => {
            for o in v["outputs"].as_array().into_iter().flatten() {
                println!("{}", o.as_str().unwrap_or_default());
            }
            for f in v["failed"].as_array().into_iter().flatten() {
                eprintln!("failed: {}: {}", f["input"].as_str().unwrap_or_default(), f["error"].as_str().unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(e) if json => {
            println!("{}", serde_json::json!({ "error": e }));
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn serve_mcp(args: &[String]) -> ExitCode {
    let (mut allow, mut all) = (Vec::new(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--allow" => match it.next() {
                Some(d) => allow.push(PathBuf::from(d)),
                None => {
                    eprintln!("--allow needs a folder");
                    return ExitCode::from(2);
                }
            },
            "--allow-all" => all = true,
            other => {
                eprintln!("unexpected argument '{other}'\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    mcp::serve(&mcp::Sandbox::new(&allow, all));
    ExitCode::SUCCESS
}
