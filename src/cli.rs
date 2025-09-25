use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "p2p-share", version, author, about = "P2P file share (Phase 1)")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Prepare to send a file: shows pairing code, connect string, and QR
    Send {
        /// Path to the file to send
        path: String,
        /// Optional pairing code; if omitted, a code is generated
        #[arg(long)]
        code: Option<String>,
        /// Print an ASCII QR for the connect string
        #[arg(long)]
        qr: bool,
        /// Use 4 words for the pairing code instead of 3
        #[arg(long)]
        strong: bool,
    },
    /// Parse a connect string and show preview. (No transfer in Phase 1)
    Recv {
        /// The connect string copied from the sender
        connect: String,
        /// Pairing code (must match the sender)
        #[arg(long)]
        code: Option<String>,
        /// Destination path (not used in Phase 1; reserved for Phase 2)
        #[arg(short, long)]
        output: Option<String>,
    },
}

