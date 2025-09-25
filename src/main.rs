mod cli;
mod code;
mod connect_string;
mod qr;

use anyhow::{bail, Context, Result};
use cli::{Cli, Cmd};
use clap::{Parser};
use code::{generate_code, normalize_code};
use connect_string::{build_connect_string, parse_connect_string, ConnectMeta};
use rand::RngCore;
use std::fs;
use std::path::PathBuf;

fn gen_salt() -> [u8; 16] {
    let mut s = [0u8; 16];
    rand::rng().fill_bytes(&mut s);
    s
}

fn read_file_meta(path: &PathBuf) -> Result<ConnectMeta> {
    let md = fs::metadata(path)
        .with_context(|| format!("Failed to stat '{}'", path.display()))?;
    if !md.is_file() {
        bail!("'{}' is not a file", path.display());
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let size = md.len();
    Ok(ConnectMeta { name, size })
}

fn cmd_send(
    path: PathBuf,
    code_opt: Option<String>,
    qr_flag: bool,
    strong: bool,
) -> Result<()> {
    let meta = read_file_meta(&path)?;
    let code = match code_opt {
        Some(c) => normalize_code(&c),
        None => {
            let words = if strong { 4 } else { 3 };
            generate_code(words)
        }
    };

    let salt = gen_salt();
    let conn = build_connect_string(Some(meta), &salt)?;

    println!("Your pairing code:");
    println!("  {}\n", code);

    println!("Connect string (give this to receiver):");
    println!("  {}\n", conn);

    if qr_flag {
        println!("QR (scan with phone camera):");
        qr::print_qr(&conn)?;
        println!();
    }

    println!("Receiver command example:");
    println!("  p2p-share recv '{}' --code '{}'\n", conn, code);

    Ok(())
}

fn cmd_recv(connect: String, code_opt: Option<String>) -> Result<()> {
    let code = match code_opt {
        Some(c) => normalize_code(&c),
        None => bail!("--code is required (Phase 1: no networking yet)"),
    };

    let bundle = parse_connect_string(&connect)?;
    println!("Pairing code entered: {}", code);

    if let Some(meta) = bundle.meta {
        println!("Incoming file preview:");
        println!("  Name: {}", meta.name);
        println!("  Size: {} bytes", meta.size);
    } else {
        println!("No file metadata found in connect string.");
    }

    println!("\nParsed connect string OK. Networking will be added in Phase 2.");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Send {
            path,
            code,
            qr,
            strong,
        } => cmd_send(path.into(), code, qr, strong),
        Cmd::Recv { connect, code, .. } => cmd_recv(connect, code),
    }
}