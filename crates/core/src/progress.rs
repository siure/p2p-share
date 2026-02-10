use indicatif::{ProgressBar, ProgressStyle};

/// Create a progress bar configured for file transfer display.
pub fn transfer_progress_bar(total_bytes: u64) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
                 {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
            )
            .expect("invalid progress bar template")
            .progress_chars("=>-"),
    );
    pb
}
