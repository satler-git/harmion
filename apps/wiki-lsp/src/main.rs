//! `wiki-lsp` entry point: stdio only.

use std::io::{self, BufReader};
use std::path::PathBuf;

use harmion_wiki_lsp::{DEFAULT_SOCKET, LspServer};

fn main() {
    let socket = match parse_args() {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("wiki-lsp: {error}");
            std::process::exit(2);
        }
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    let server = LspServer::new(BufReader::new(stdin.lock()), stdout.lock(), socket);

    match server.run() {
        Ok((code, _writer)) => std::process::exit(code),
        Err(error) => {
            eprintln!("wiki-lsp: {error}");
            std::process::exit(1);
        }
    }
}

fn parse_args() -> Result<PathBuf, String> {
    let mut socket = std::env::var_os("WIKI_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET));

    let mut args = std::env::args().skip(1).peekable();
    // Accept `wiki-lsp lsp --stdio` as well as the plugin's direct binary.
    if args.peek().map(String::as_str) == Some("lsp") {
        args.next();
    }

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--stdio" => {}
            "--socket" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--socket requires a path".to_owned())?;
                socket = PathBuf::from(value);
            }
            _ if argument.starts_with("--socket=") => {
                let value = &argument["--socket=".len()..];
                if value.is_empty() {
                    return Err("--socket requires a path".to_owned());
                }
                socket = PathBuf::from(value);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("wiki-lsp {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(socket)
}

fn print_usage() {
    println!(
        "wiki-lsp {}\n\n\
         Usage: wiki-lsp [--stdio] [--socket PATH]\n\n\
         A stdio Language Server for wiki://<content-id> buffers.\n\
         The socket path can also come from initializationOptions.socket\n\
         (used by the Neovim plugin) or the WIKI_SOCKET environment variable.\n\
         Default: {}",
        env!("CARGO_PKG_VERSION"),
        DEFAULT_SOCKET
    );
}
