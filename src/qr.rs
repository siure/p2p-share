use anyhow::Result;
use qrcodegen::{QrCode, QrCodeEcc};

pub fn print_qr(text: &str) -> Result<()> {
    let code = QrCode::encode_text(text, QrCodeEcc::Medium)?;
    // Add a small quiet zone border
    let border: i32 = 2;
    let size = code.size();
    for y in -border..size + border {
        let mut line = String::with_capacity((size as usize + 2) * 2);
        for x in -border..size + border {
            let dark = code.get_module(x, y);
            // Two-character blocks look nicer in terminals
            if dark {
                line.push_str("██");
            } else {
                line.push_str("  ");
            }
        }
        println!("{}", line);
    }
    Ok(())
}