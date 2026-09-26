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

/// Limits UI work independently of transfer speed, while preserving completion.
#[derive(Default)]
pub struct ProgressThrottle {
    last: Option<std::time::Instant>,
}

impl ProgressThrottle {
    pub fn should_emit(&mut self, done: u64, total: u64) -> bool {
        let now = std::time::Instant::now();
        if done >= total
            || self.last.is_none_or(|last| {
                now.duration_since(last) >= std::time::Duration::from_millis(100)
            })
        {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProgressThrottle;

    #[test]
    fn progress_preserves_first_and_final_updates() {
        let mut throttle = ProgressThrottle::default();
        assert!(throttle.should_emit(1, 100));
        assert!(!throttle.should_emit(2, 100));
        assert!(throttle.should_emit(100, 100));
    }
}
