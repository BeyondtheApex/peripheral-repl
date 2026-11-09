use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use peripheral_repl::{bindings, protocol, runtime::Runtime};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "Lua peripheral runtime. Feature and output reports require exact device policies."
)]
struct Cli {
    #[arg(long, default_value = ".", global = true)]
    root: PathBuf,
    #[arg(long, global = true)]
    policy: Option<PathBuf>,
    /// Connect to a running daemon through its protected endpoint file.
    #[arg(long, global = true)]
    connect: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Interactive Lua shell; :help lists management commands.
    Shell,
    /// One JSON command per line, one JSON result per line. No stdout banners.
    Json,
    /// MCP JSON-RPC over stdio, optionally bridged to --connect.
    Mcp,
    /// Local authenticated daemon (loopback, ephemeral port).
    Serve {
        #[arg(long)]
        endpoint: Option<PathBuf>,
        #[arg(long)]
        script: Option<String>,
        #[arg(long, default_value = "default")]
        instance: String,
    },
    /// Send a single JSON command; requires --connect.
    Request { command: String },
    /// Run a Lua file once and print structured results.
    Run {
        path: String,
        #[arg(long, default_value = "default")]
        instance: String,
    },
    /// Enumerate HID collections without opening them.
    Devices,
    /// Bluetooth LE discovery; never connects or pairs automatically.
    Bluetooth {
        #[arg(default_value = "adapters", value_parser = ["adapters", "list", "scan"])]
        action: String,
        #[arg(long, default_value_t = 3000)]
        duration_ms: u64,
        #[arg(long, default_value_t = 0)]
        adapter: usize,
    },
    /// Stream daemon events as JSON Lines until interrupted.
    Watch {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--native-hid-worker") {
        return peripheral_repl::native_hid::serve();
    }
    let cli = Cli::parse();
    let root = cli.root.canonicalize()?;
    if cli.connect.is_some() && cli.policy.is_some() {
        bail!("policy is fixed by the daemon; do not combine --policy and --connect");
    }
    let mut runtime = Runtime::new(&root)?;
    if let Some(path) = cli.policy {
        std::sync::Arc::get_mut(&mut runtime.shared).unwrap().policy =
            bindings::load_policy(&path)?;
    }
    let command = cli.command.unwrap_or(Command::Shell);
    if let Command::Serve {
        endpoint,
        script,
        instance,
    } = command
    {
        if cli.connect.is_some() {
            bail!("serve cannot use --connect");
        }
        runtime.dispatch(json!({"op":"create","instance":instance}))?;
        let startup = script
            .map(|path| json!({"op":"load","instance":instance,"path":path,"timeout_ms":30000}));
        return protocol::serve(
            runtime,
            &endpoint.unwrap_or_else(protocol::default_endpoint),
            root,
            startup,
        );
    }
    let connected = cli.connect.is_some();
    if matches!(command, Command::Mcp) {
        return protocol::mcp_stdio(runtime, cli.connect);
    }
    if let Command::Watch { after } = command {
        return protocol::watch(
            cli.connect
                .as_deref()
                .ok_or_else(|| anyhow!("watch requires --connect"))?,
            after,
        );
    }
    let mut dispatch = |command: Value| -> Result<Value> {
        if let Some(path) = &cli.connect {
            protocol::request(path, command)
        } else {
            Ok(protocol::envelope(&runtime, command))
        }
    };
    match command {
        Command::Json | Command::Mcp => {
            let mcp_mode = matches!(command, Command::Mcp);
            let mut input = io::stdin().lock();
            let mut output = io::stdout().lock();
            while let Some(line) = protocol::read_frame(&mut input)? {
                let response = match serde_json::from_str::<Value>(&line) {
                    Ok(req) if mcp_mode => protocol::mcp(req, &mut dispatch),
                    Ok(req) => Some(dispatch(req)?),
                    Err(e) if mcp_mode => Some(
                        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}}),
                    ),
                    Err(e) => Some(json!({"id":null,"ok":false,"error":{"message":e.to_string()}})),
                };
                if let Some(response) = response {
                    serde_json::to_writer(&mut output, &response)?;
                    output.write_all(b"\n")?;
                    output.flush()?;
                }
            }
        }
        Command::Request { command } => {
            if !connected {
                bail!("request requires --connect");
            }
            print_result(dispatch(serde_json::from_str(&command)?)?)?;
        }
        Command::Run { path, instance } => {
            if !connected {
                check(dispatch(json!({"op":"create","instance":instance}))?)?;
            }
            print_result(dispatch(
                json!({"op":"load","instance":instance,"path":path,"timeout_ms":30000}),
            )?)?;
        }
        Command::Devices => print_result(dispatch(json!({"op":"devices"}))?)?,
        Command::Bluetooth {
            action,
            duration_ms,
            adapter,
        } => print_result(dispatch(
            json!({"op":"bluetooth","action":action,"duration_ms":duration_ms,"adapter":adapter}),
        )?)?,
        Command::Watch { .. } => unreachable!(),
        Command::Shell => shell(&mut dispatch)?,
        Command::Serve { .. } => unreachable!(),
    }
    Ok(())
}

fn check(v: Value) -> Result<Value> {
    if v["ok"] != true {
        bail!(
            "{}",
            v["error"]["message"].as_str().unwrap_or("command failed")
        );
    }
    Ok(v["result"].clone())
}
fn print_result(v: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&v)?);
    if v["ok"] != true {
        bail!("command failed");
    }
    Ok(())
}

fn shell(dispatch: &mut impl FnMut(Value) -> Result<Value>) -> Result<()> {
    let mut instance = "default".to_string();
    let names = check(dispatch(json!({"op":"instances"}))?)?;
    if !names
        .as_array()
        .is_some_and(|a| a.contains(&json!(instance)))
    {
        check(dispatch(json!({"op":"create","instance":instance}))?)?;
    }
    let mut editor = rustyline::Editor::<
        peripheral_repl::repl::ShellHelper,
        rustyline::history::DefaultHistory,
    >::new()?;
    editor.set_helper(Some(peripheral_repl::repl::ShellHelper::default()));
    let _ = editor.load_history(".peripheral.history");
    eprintln!("Peripheral REPL — :help for commands, :begin / :end for multiline Lua");
    let mut multiline: Option<String> = None;
    loop {
        let prompt = if multiline.is_some() {
            "...> ".to_string()
        } else {
            format!("{instance}> ")
        };
        editor.helper_mut().unwrap().manual_multiline = multiline.is_some();
        let line = match editor.readline(&prompt) {
            Ok(s) => s,
            Err(rustyline::error::ReadlineError::Interrupted) => {
                multiline = None;
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        let _ = editor.add_history_entry(&line);
        if line.trim() == ":quit" {
            break;
        }
        if line.trim() == ":begin" {
            multiline = Some(String::new());
            continue;
        }
        if let Some(code) = multiline.as_mut()
            && line.trim() != ":end"
        {
            if code.len() + line.len() > peripheral_repl::runtime::MAX_SOURCE {
                eprintln!("source limit exceeded");
                multiline = None;
            } else {
                code.push_str(&line);
                code.push('\n');
            }
            continue;
        }
        let line = multiline.take().unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        if line.trim() == ":help" {
            println!(
                ":new NAME, :use NAME, :instances, :devices, :bluetooth adapters|list|scan, :load PATH, :reload PATH\n:tasks, :cancel TASK_ID, :stats, :snapshot, :events, :pause, :resume, :start, :stop, :destroy\n:actions, :invoke NAME JSON, :config, :configure JSON, :lifecycle\n:begin / :end for multiline Lua; :quit. Lua APIs: ble.*, runtime.*, buffer.*, errors.*, events.*, log.*, config.*, lifecycle.*, storage.*"
            );
            continue;
        }
        let request = if let Some(command) = line.strip_prefix(':') {
            let (op, arg) = command.split_once(' ').unwrap_or((command, ""));
            let arg = arg.trim();
            match op {
                "new" => json!({"op":"create","instance":arg}),
                "use" => {
                    let names = check(dispatch(json!({"op":"instances"}))?)?;
                    if names.as_array().is_some_and(|a| a.contains(&json!(arg))) {
                        instance = arg.to_owned();
                    } else {
                        eprintln!("unknown instance: {arg}");
                    }
                    continue;
                }
                "load" | "reload" => json!({"op":op,"instance":instance,"path":arg}),
                "bluetooth" => {
                    json!({"op":op,"action":if arg.is_empty(){"adapters"}else{arg},"duration_ms":3000})
                }
                "cancel" => match arg.parse::<u64>() {
                    Ok(task) => json!({"op":op,"instance":instance,"task":task}),
                    Err(e) => {
                        eprintln!("{e}");
                        continue;
                    }
                },
                "configure" => match serde_json::from_str::<Value>(arg) {
                    Ok(value) => json!({"op":op,"instance":instance,"value":value}),
                    Err(e) => {
                        eprintln!("{e}");
                        continue;
                    }
                },
                "invoke" => {
                    let (name, args) = arg.split_once(' ').unwrap_or((arg, "{}"));
                    match serde_json::from_str::<Value>(args) {
                        Ok(value) => {
                            json!({"op":op,"instance":instance,"name":name,"arguments":value})
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            continue;
                        }
                    }
                }
                _ => json!({"op":op,"instance":instance}),
            }
        } else {
            json!({"op":"eval","instance":instance,"code":line})
        };
        let creating = request["op"] == "create";
        let new_name = request["instance"].as_str().unwrap_or("").to_string();
        match dispatch(request).and_then(check) {
            Ok(value) => {
                if creating {
                    instance = new_name;
                }
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            Err(e) => eprintln!("{e}"),
        }
    }
    editor
        .save_history(".peripheral.history")
        .map_err(|e| anyhow!(e))?;
    Ok(())
}
