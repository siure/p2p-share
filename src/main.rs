mod crypto;
mod progress;
mod protocol;
mod receiver;
mod sender;
mod ticket;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// p2p-share — simple peer-to-peer file transfer.
///
/// Uses iroh for automatic NAT traversal (UPnP, hole-punching, relay),
/// so no manual port opening is required.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send a file to another device.
    Send {
        /// Path to the file to send.
        file: PathBuf,

        /// Connect to a waiting receiver instead of waiting for one.
        /// Use the ticket shown by `p2p-share receive --qr`.
        #[arg(long)]
        to: Option<String>,
    },

    /// Receive a file from another device.
    Receive {
        /// Connection ticket (shown by the sender) or ip:port for direct LAN.
        /// Not required when using --qr.
        target: Option<String>,

        /// Directory to save the received file in.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,

        /// Listen mode: create an endpoint, display a QR code, and wait for
        /// a sender to connect with `p2p-share send --to <ticket>`.
        /// Useful when the sender is a phone and typing long tickets is impractical.
        #[arg(long)]
        qr: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Send { file, to: None } => sender::run(&file).await,
        Command::Send { file, to: Some(ref ticket) } => sender::run_reverse(&file, ticket).await,
        Command::Receive { target: _, output, qr: true } => receiver::run_listen(&output).await,
        Command::Receive { target: Some(ref target), output, qr: false } => {
            receiver::run(target, &output).await
        }
        Command::Receive { target: None, output: _, qr: false } => {
            eprintln!("Error: either provide a <TARGET> ticket/address, or use --qr to listen.");
            eprintln!();
            eprintln!("Examples:");
            eprintln!("  p2p-share receive p2psh:XXXXX        # connect to a sender");
            eprintln!("  p2p-share receive --qr               # wait for a sender (shows QR)");
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
