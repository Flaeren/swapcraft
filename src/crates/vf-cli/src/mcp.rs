//! `voidflare-cli mcp`: a Model Context Protocol server on stdin/stdout, so an LLM can use every
//! voidflare tool without a window. Newline-delimited JSON-RPC 2.0, protocol revision 2025-06-18 (older
//! clients are answered with the revision they asked for when we know it).
//!
//! Safety: a model must not be able to read or write the whole disk. Every path it passes is resolved
//! (links followed) and must sit inside an allowed folder: the folder the server was started in, plus each
//! `--allow DIR`. `--allow-all` lifts the limit and has to be asked for by name.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::api;

const LATEST: &str = "2025-06-18";
const KNOWN: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

pub struct Sandbox {
    roots: Vec<PathBuf>,
    all: bool,
}

/// `\\?\C:\x` is what canonicalize gives on Windows; people (and models) want `C:\x`.
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

impl Sandbox {
    pub fn new(allow: &[PathBuf], all: bool) -> Self {
        let mut roots: Vec<PathBuf> = allow.iter().filter_map(|p| p.canonicalize().ok()).map(plain).collect();
        if roots.is_empty()
            && !all
            && let Ok(cwd) = std::env::current_dir()
        {
            roots.push(plain(cwd.canonicalize().unwrap_or(cwd)));
        }
        Sandbox { roots, all }
    }

    fn inside(&self, p: &Path) -> bool {
        self.all || self.roots.iter().any(|r| p.starts_with(r))
    }

    fn denied(&self, p: &Path) -> String {
        let roots: Vec<String> = self.roots.iter().map(|r| r.display().to_string()).collect();
        format!("{} is outside the folders this server may use ({}). Start it with --allow <folder> to add one.", p.display(), roots.join(", "))
    }

    /// A file that exists, inside the sandbox.
    pub fn file(&self, raw: &str) -> Result<PathBuf, String> {
        let p = plain(Path::new(raw).canonicalize().map_err(|e| format!("{raw}: {e}"))?);
        if !self.inside(&p) {
            return Err(self.denied(&p));
        }
        Ok(p)
    }

    /// A folder to write into (it may not exist yet), inside the sandbox.
    pub fn out_dir(&self, raw: &str) -> Result<PathBuf, String> {
        let abs = std::path::absolute(raw).map_err(|e| format!("{raw}: {e}"))?;
        // Resolve the part that exists (so a link cannot lead out), keep the rest as written.
        let mut existing = abs.as_path();
        let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
        while !existing.exists() {
            let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else { break };
            tail.push(name);
            existing = parent;
        }
        let mut resolved = plain(existing.canonicalize().map_err(|e| format!("{raw}: {e}"))?);
        for part in tail.into_iter().rev() {
            if part == ".." {
                return Err(format!("{raw}: '..' is not allowed in a folder that does not exist yet"));
            }
            resolved.push(part);
        }
        if !self.inside(&resolved) {
            return Err(self.denied(&resolved));
        }
        Ok(resolved)
    }
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": props, "required": required } })
}

fn tools() -> Vec<Value> {
    let out_dir = json!({ "type": "string", "description": "Folder for the result. Default: next to the source file." });
    vec![
        tool(
            "voidflare_formats",
            "List every file type voidflare can convert and what each can become (the 'to' ids to pass to voidflare_convert). Call this first when unsure.",
            json!({}),
            &[],
        ),
        tool(
            "voidflare_convert",
            "Convert one file offline (documents, spreadsheets, slides, PDF, DWG drawings and more). Returns the path of the new file. Never overwrites: an existing name gets ' (2)'.",
            json!({
                "path": { "type": "string", "description": "Absolute path of the file to convert." },
                "to": { "type": "string", "description": "Target id from voidflare_formats, e.g. 'pdf', 'docx', 'png', 'pdf-sheets' (DWG sheets merged), 'pdf-whole' (DWG whole drawing)." },
                "out_dir": out_dir
            }),
            &["path", "to"],
        ),
        tool(
            "voidflare_pdf_merge",
            "Join two or more PDF files, in the order given, into one PDF.",
            json!({
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths of the PDFs, in order." },
                "name": { "type": "string", "description": "Name of the merged file. Default 'merged.pdf'." },
                "out_dir": out_dir
            }),
            &["paths"],
        ),
        tool(
            "voidflare_pdf_split",
            "Cut a PDF into parts: a new file every N pages ('every') or starting at given pages ('before'). Give exactly one. Returns the folder holding the parts.",
            json!({
                "path": { "type": "string" },
                "every": { "type": "integer", "minimum": 1, "description": "Pages per part." },
                "before": { "type": "string", "description": "Pages where a new part starts, e.g. \"3,7\"." },
                "out_dir": out_dir
            }),
            &["path"],
        ),
        tool(
            "voidflare_pdf_extract",
            "Keep only some pages of a PDF in a new file.",
            json!({ "path": { "type": "string" }, "pages": { "type": "string", "description": "Pages and ranges, e.g. \"1,3,5-8\"." }, "out_dir": out_dir }),
            &["path", "pages"],
        ),
        tool(
            "voidflare_pdf_ocr",
            "Make a scanned PDF searchable (OCR). English only: accents in other languages come out wrong.",
            json!({ "path": { "type": "string" }, "out_dir": out_dir }),
            &["path"],
        ),
        tool(
            "voidflare_pdf_shrink",
            "Make a PDF smaller by recompressing its images.",
            json!({
                "path": { "type": "string" },
                "ppi": { "type": "integer", "minimum": 30, "maximum": 600, "description": "Image resolution. Default 150." },
                "quality": { "type": "integer", "minimum": 10, "maximum": 100, "description": "JPEG quality. Default 60." },
                "out_dir": out_dir
            }),
            &["path"],
        ),
        tool(
            "voidflare_info",
            "Size and type of a file, what it can be converted to, and for a PDF its page count and properties.",
            json!({ "path": { "type": "string" } }),
            &["path"],
        ),
        tool("voidflare_engines", "Which conversion engines voidflare found on this PC, with versions. Use it to find out why a conversion failed.", json!({}), &[]),
    ]
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| format!("missing '{key}'"))
}

fn opt_out(sb: &Sandbox, args: &Value) -> Result<Option<PathBuf>, String> {
    match args.get("out_dir").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(d) => sb.out_dir(d).map(Some),
        None => Ok(None),
    }
}

fn uint(args: &Value, key: &str, default: u32, min: u32, max: u32) -> Result<u32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().filter(|n| (min as u64..=max as u64).contains(n)).map(|n| n as u32).ok_or_else(|| format!("'{key}' must be a whole number from {min} to {max}")),
    }
}

/// Runs one tool. `Err` becomes a tool result with `isError: true`, so the model can read it and try again.
pub fn call(sb: &Sandbox, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "voidflare_formats" => Ok(api::formats()),
        "voidflare_engines" => Ok(api::engines()),
        "voidflare_convert" => {
            let (file, to) = (sb.file(text_arg(args, "path")?)?, text_arg(args, "to")?);
            api::convert(&file, to, opt_out(sb, args)?.as_deref())
        }
        "voidflare_pdf_merge" => {
            let list = args.get("paths").and_then(Value::as_array).ok_or("missing 'paths'")?;
            let files = list.iter().map(|v| v.as_str().ok_or_else(|| "'paths' must be strings".to_owned()).and_then(|s| sb.file(s))).collect::<Result<Vec<_>, _>>()?;
            let name = args.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("merged.pdf");
            if name.contains(['/', '\\']) {
                return Err("'name' is a file name, not a path: use out_dir for the folder".to_owned());
            }
            api::pdf_merge(&files, name, opt_out(sb, args)?.as_deref())
        }
        "voidflare_pdf_split" => {
            let file = sb.file(text_arg(args, "path")?)?;
            let every = match args.get("every") {
                None | Some(Value::Null) => None,
                Some(_) => Some(uint(args, "every", 1, 1, 100_000)?),
            };
            api::pdf_split(&file, every, args.get("before").and_then(Value::as_str), opt_out(sb, args)?.as_deref())
        }
        "voidflare_pdf_extract" => {
            let file = sb.file(text_arg(args, "path")?)?;
            api::pdf_extract(&file, text_arg(args, "pages")?, opt_out(sb, args)?.as_deref())
        }
        "voidflare_pdf_ocr" => api::pdf_ocr(&sb.file(text_arg(args, "path")?)?, opt_out(sb, args)?.as_deref()),
        "voidflare_pdf_shrink" => {
            let file = sb.file(text_arg(args, "path")?)?;
            api::pdf_shrink(&file, uint(args, "ppi", 150, 30, 600)?, uint(args, "quality", 60, 10, 100)?, opt_out(sb, args)?.as_deref())
        }
        "voidflare_info" => api::info(&sb.file(text_arg(args, "path")?)?),
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answers one JSON-RPC message; `None` for a notification (which gets no answer).
pub fn handle(sb: &Sandbox, msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str)?;
    let id = msg.get("id")?; // no id = notification
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST);
            let version = if KNOWN.contains(&asked) { asked } else { LATEST };
            reply(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "voidflare", "title": "voidflare", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "voidflare converts files offline on this PC. Call voidflare_formats to see what converts to what, then voidflare_convert. Paths must be absolute and inside the folders the server was allowed."
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => reply(id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let (text, is_error) = match call(sb, name, &args) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                Err(e) => (e, true),
            };
            reply(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        other => failure(id, -32601, &format!("method not found: {other}")),
    })
}

/// Reads messages from stdin until it closes.
pub fn serve(sb: &Sandbox) {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(_)) => Some(failure(&Value::Null, -32600, "batches are not supported")),
            Ok(msg) => handle(sb, &msg),
            Err(e) => Some(failure(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(a) = answer {
            let _ = writeln!(out, "{a}");
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox_in(dir: &Path) -> Sandbox {
        Sandbox::new(&[dir.to_path_buf()], false)
    }

    #[test]
    fn handshake_lists_tools_and_ignores_notifications() {
        let sb = Sandbox::new(&[], false);
        let init = handle(&sb, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["result"]["serverInfo"]["name"], "voidflare");
        assert!(handle(&sb, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let list = handle(&sb, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"voidflare_convert") && names.contains(&"voidflare_pdf_merge"));
        let unknown = handle(&sb, &json!({"jsonrpc":"2.0","id":3,"method":"nope"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let odd = handle(&sb, &json!({"jsonrpc":"2.0","id":4,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).unwrap();
        assert_eq!(odd["result"]["protocolVersion"], LATEST);
    }

    #[test]
    fn the_sandbox_keeps_paths_inside() {
        let base = std::env::temp_dir().join(format!("vf_mcp_sandbox_{}", std::process::id()));
        let (allowed, other) = (base.join("allowed"), base.join("other"));
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(allowed.join("a.txt"), b"x").unwrap();
        std::fs::write(other.join("b.txt"), b"x").unwrap();
        let sb = sandbox_in(&allowed);
        assert!(sb.file(allowed.join("a.txt").to_str().unwrap()).is_ok());
        assert!(sb.file(other.join("b.txt").to_str().unwrap()).is_err(), "a file outside is refused");
        let sneaky = format!("{}/../other/b.txt", allowed.display());
        assert!(sb.file(&sneaky).is_err(), "'..' cannot lead out");
        assert!(sb.out_dir(allowed.join("new/deeper").to_str().unwrap()).is_ok(), "a folder to create is fine");
        assert!(sb.out_dir(other.to_str().unwrap()).is_err());
        assert!(sb.out_dir(&format!("{}/new/../../other", allowed.display())).is_err());
        assert!(Sandbox::new(&[], true).file(other.join("b.txt").to_str().unwrap()).is_ok(), "--allow-all lifts the limit");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_bad_call_is_a_tool_error_the_model_can_read() {
        let sb = Sandbox::new(&[], false);
        let r = handle(&sb, &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"voidflare_convert","arguments":{}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("path"));
    }
}
