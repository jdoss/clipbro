use crate::entry::Entry;

pub fn copy_to_clipboard(entry: &Entry) {
    if let Some(text) = entry.text_content() {
        let text = text.to_string();
        std::thread::spawn(move || {
            for args in [vec![], vec!["--primary"]] {
                let result = std::process::Command::new("wl-copy")
                    .args(&args)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            stdin.write_all(text.as_bytes())?;
                        }
                        child.wait()
                    });
                if let Err(e) = result {
                    tracing::error!("wl-copy failed: {e}");
                }
            }
        });
    } else if let Some((mime, data)) = entry.image_data() {
        let mime = mime.to_string();
        let data = data.to_vec();
        std::thread::spawn(move || {
            for args in [
                vec!["--type", &mime],
                vec!["--primary", "--type", &mime],
            ] {
                let result = std::process::Command::new("wl-copy")
                    .args(&args)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            stdin.write_all(&data)?;
                        }
                        child.wait()
                    });
                if let Err(e) = result {
                    tracing::error!("wl-copy failed: {e}");
                }
            }
        });
    }
}
