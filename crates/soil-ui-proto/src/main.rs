// SPDX-License-Identifier: MIT OR Apache-2.0
//! A throwaway host for the Lisp -> HTML/CSS UI examples.
//!
//! Every example exports the same four functions:
//!
//! ```text
//! (initial-state)            -> value
//! (update state action)      -> value
//! (view state)               -> UI form
//! (styles)                   -> CSS text
//! ```
//!
//! The action is either a plain name (`"inc"`) or a JSON object string
//! (`{"op":"add","text":"buy milk"}`). The examples use the JSON form for
//! payloads; the host does not care.
//!
//! This is a dev server, not the real `soil:ui` host. It exists to keep the
//! examples editable while the contract is still moving.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use soil_lang::sexp::Value;
use soil_lang::{FakeHost, Runtime, from_json, to_json};

const INDEX_HTML: &str = include_str!("../web/index.html");
const UI_JS: &str = include_str!("../web/ui.js");
const MAX_BODY: usize = 1 << 20;

struct App {
    source: PathBuf,
    examples: PathBuf,
    stdlib_prelude: Option<PathBuf>,
    ui_prelude: Option<PathBuf>,
    extra_prelude: Option<PathBuf>,
}

fn main() -> Result<()> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut examples = crate_dir.join("../../examples/ui");
    let mut source = None;
    let mut stdlib_prelude = Some(crate_dir.join("../soil-lang/stdlib/prelude.soil"));
    let mut extra_prelude = None;
    let mut port = 7878u16;
    let mut render = false;
    let mut state = None;
    let mut action = None;
    let mut name = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => source = Some(PathBuf::from(next(&mut args, "--source")?)),
            "--examples" => examples = PathBuf::from(next(&mut args, "--examples")?),
            "--prelude" => extra_prelude = Some(PathBuf::from(next(&mut args, "--prelude")?)),
            "--no-prelude" => stdlib_prelude = None,
            "--name" => name = Some(next(&mut args, "--name")?),
            "--port" => port = next(&mut args, "--port")?.parse()?,
            "--state" => state = Some(parse_state(&next(&mut args, "--state")?)?),
            "--action" => action = Some(next(&mut args, "--action")?),
            "--render" => render = true,
            "-h" | "--help" => {
                println!(
                    "usage: soil-ui-proto [--source FILE.soil] [--examples DIR] [--name EXAMPLE] \
                     [--prelude FILE] [--no-prelude] [--port N] \
                     [--render [--state N] [--action ACTION]]"
                );
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }

    let source = source.unwrap_or_else(|| examples.join("counter.soil"));
    let ui_prelude = Some(examples.join("prelude.soil"));
    let app = Arc::new(App {
        source,
        examples,
        stdlib_prelude,
        ui_prelude,
        extra_prelude,
    });

    if render {
        let source = select_source(&app, name.as_deref())?;
        let value = render_json(&app, &source, state, action.as_deref())?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let listener =
        TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("bind 127.0.0.1:{port}"))?;
    println!("soil-ui-proto: http://127.0.0.1:{port}/");
    println!("examples: {}", app.examples.display());
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("accept: {error}");
                continue;
            }
        };
        let app = Arc::clone(&app);
        // The evaluator, HTML renderer, and JSON encoder are recursive. A
        // browser request gets the same headroom as the CLI, not the 2 MiB
        // default thread stack.
        let spawned = std::thread::Builder::new()
            .name("soil-ui-request".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                if let Err(error) = handle(stream, &app) {
                    eprintln!("request: {error:#}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("spawn request: {error}");
        }
    }
    Ok(())
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
}

fn select_source(app: &App, name: Option<&str>) -> Result<PathBuf> {
    match name {
        Some(name) if !name.is_empty() => {
            let stem = name.strip_suffix(".soil").unwrap_or(name);
            if stem.is_empty() || stem.contains('/') || stem.contains('\\') || stem.contains("..") {
                bail!("invalid example name `{name}`");
            }
            let path = app.examples.join(format!("{stem}.soil"));
            if !path.is_file() {
                bail!("unknown example `{name}`");
            }
            Ok(path)
        }
        _ => Ok(app.source.clone()),
    }
}

fn build_runtime(app: &App, source: &Path) -> Result<Runtime> {
    let mut runtime = Runtime::new(Box::new(FakeHost::new().deny_all()));
    if let Some(prelude) = &app.stdlib_prelude {
        runtime
            .load_file(prelude)
            .with_context(|| format!("load prelude {}", prelude.display()))?;
        if let Some(directory) = prelude.parent() {
            for name in ["ui.soil", "highlight.soil"] {
                let path = directory.join(name);
                if path.is_file() {
                    runtime
                        .load_file(&path)
                        .with_context(|| format!("load stdlib {}", path.display()))?;
                }
            }
        }
    }
    if let Some(prelude) = &app.ui_prelude
        && prelude.is_file()
    {
        runtime
            .load_file(prelude)
            .with_context(|| format!("load UI prelude {}", prelude.display()))?;
    }
    if let Some(prelude) = &app.extra_prelude {
        runtime
            .load_file(prelude)
            .with_context(|| format!("load prelude {}", prelude.display()))?;
    }
    runtime
        .load_file(source)
        .with_context(|| format!("load {}", source.display()))?;
    Ok(runtime)
}

fn parse_state(raw: &str) -> Result<Value> {
    if let Ok(number) = raw.parse::<i64>() {
        return Ok(Value::Int(number));
    }
    let json: serde_json::Value = serde_json::from_str(raw)
        .with_context(|| format!("state is neither an integer nor JSON: {raw}"))?;
    Ok(from_json(&json))
}

fn render_json(
    app: &App,
    source: &Path,
    state: Option<Value>,
    action: Option<&str>,
) -> Result<serde_json::Value> {
    let mut runtime = build_runtime(app, source)?;

    let mut state = match state {
        Some(value) => value,
        None => runtime
            .call("initial-state", &[])
            .context("call (initial-state)")?,
    };

    if let Some(action) = action {
        state = runtime
            .call("update", &[state, Value::String(action.to_string())])
            .with_context(|| format!("call (update state {action:?})"))?;
    }

    let tree = runtime
        .call("view", &[state.clone()])
        .context("call (view state)")?;
    let css = runtime.call("styles", &[]).context("call (styles)")?;
    let Value::String(css) = css else {
        bail!("(styles) must return a string, got {css}");
    };

    Ok(serde_json::json!({
        "state": to_json(&state),
        "tree": to_json(&tree),
        "css": css,
    }))
}

fn handle(mut stream: TcpStream, app: &App) -> Result<()> {
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_BODY {
        return respond(&mut stream, 413, "text/plain", b"body too large");
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;

    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let name = query_param(query, "name");

    match (method, path) {
        ("GET", "/") => respond(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes(),
        ),
        ("GET", "/ui.js") => respond(
            &mut stream,
            200,
            "text/javascript; charset=utf-8",
            UI_JS.as_bytes(),
        ),
        ("GET", "/examples") => match examples_json(app) {
            Ok(value) => {
                let body = serde_json::to_vec(&value)?;
                respond(&mut stream, 200, "application/json", &body)
            }
            Err(error) => respond_error(&mut stream, 500, &format!("{error:#}")),
        },
        ("GET", "/source") => match select_source(app, name.as_deref()) {
            Ok(source) => match fs::read(&source) {
                Ok(bytes) => respond(&mut stream, 200, "text/plain; charset=utf-8", &bytes),
                Err(error) => respond_error(&mut stream, 500, &format!("read source: {error}")),
            },
            Err(error) => respond_error(&mut stream, 404, &format!("{error:#}")),
        },
        ("POST", "/source") => match select_source(app, name.as_deref()) {
            Ok(source) => {
                let text = String::from_utf8_lossy(&body);
                match write_source(&source, &text) {
                    Ok(()) => respond(&mut stream, 200, "application/json", br#"{"ok":true}"#),
                    Err(error) => respond_error(&mut stream, 500, &format!("{error:#}")),
                }
            }
            Err(error) => respond_error(&mut stream, 404, &format!("{error:#}")),
        },
        ("GET", "/view") => {
            let state = query_param(query, "state").and_then(|value| parse_state(&value).ok());
            let action = query_param(query, "action");
            match select_source(app, name.as_deref())
                .and_then(|source| render_json(app, &source, state, action.as_deref()))
            {
                Ok(value) => {
                    let body = serde_json::to_vec(&value)?;
                    respond(&mut stream, 200, "application/json", &body)
                }
                Err(error) => respond_error(&mut stream, 422, &format!("{error:#}")),
            }
        }
        ("GET", "/health") => respond(&mut stream, 200, "text/plain", b"ok"),
        _ => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn examples_json(app: &App) -> Result<serde_json::Value> {
    let mut examples = Vec::new();
    if let Ok(entries) = fs::read_dir(&app.examples) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("soil") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem == "prelude" || stem.is_empty() {
                continue;
            }
            examples.push((stem.to_string(), title_case(stem)));
        }
    }
    examples.sort();
    Ok(serde_json::Value::Array(
        examples
            .into_iter()
            .map(|(name, title)| serde_json::json!({ "name": name, "title": title }))
            .collect(),
    ))
}

fn title_case(stem: &str) -> String {
    stem.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn write_source(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("soil.tmp");
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high * 16 + low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn respond_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    let body = serde_json::json!({ "error": message });
    respond(
        stream,
        status,
        "application/json",
        &serde_json::to_vec(&body)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let examples = crate_dir.join("../../examples/ui");
        App {
            source: examples.join("counter.soil"),
            examples: examples.clone(),
            stdlib_prelude: Some(crate_dir.join("../soil-lang/stdlib/prelude.soil")),
            ui_prelude: Some(examples.join("prelude.soil")),
            extra_prelude: None,
        }
    }

    fn render(name: &str, action: Option<&str>) -> serde_json::Value {
        let app = app();
        let source = app.examples.join(format!("{name}.soil"));
        render_json(&app, &source, None, action).unwrap()
    }

    #[test]
    fn counter_starts_at_zero_and_increments() {
        let value = render("counter", None);
        assert_eq!(value["state"], serde_json::json!(0));
        assert_eq!(value["tree"]["tag"], serde_json::json!("div"));

        let value = render("counter", Some(r#"{"op":"inc"}"#));
        assert_eq!(value["state"], serde_json::json!(1));
    }

    #[test]
    fn todo_adds_and_toggles() {
        let value = render("todo", Some(r#"{"op":"add","text":"ship it"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("ship it"));

        let value = render("todo", Some(r#"{"op":"toggle","id":"1"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("todo-text done"));
    }

    #[test]
    fn calculator_evaluates() {
        let value = render("calculator", Some(r#"{"op":"digit","value":"7"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("\"text\":\"7\""));

        let value = render("calculator", Some(r#"{"op":"op","value":"+"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("\"text\":\"7\""));
    }

    #[test]
    fn calculator_carries_state_between_requests() {
        let app = app();
        let source = app.examples.join("calculator.soil");
        let mut state = None;
        for action in [
            r#"{"op":"digit","value":"7"}"#,
            r#"{"op":"digit","value":"8"}"#,
            r#"{"op":"op","value":"+"}"#,
            r#"{"op":"digit","value":"5"}"#,
            r#"{"op":"equals"}"#,
        ] {
            let value = render_json(&app, &source, state, Some(action)).unwrap();
            state = Some(from_json(&value["state"]));
        }
        let value = render_json(&app, &source, state, None).unwrap();
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("\"text\":\"83\""));
    }

    #[test]
    fn tictactoe_rejects_taken_cells() {
        let value = render("tictactoe", Some(r#"{"op":"play","index":"4"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("\"text\":\"X\""));
        assert!(tree.contains("O to move"));
    }

    #[test]
    fn form_validates() {
        let value = render("form", Some(r#"{"op":"submit","name":"","email":"x"}"#));
        let tree = serde_json::to_string(&value["tree"]).unwrap();
        assert!(tree.contains("Name is required."));
    }

    #[test]
    fn every_example_renders() {
        for name in ["calculator", "counter", "form", "tabs", "tictactoe", "todo"] {
            let value = render(name, None);
            assert!(value["css"].as_str().unwrap().contains(".app"));
        }
    }
}
