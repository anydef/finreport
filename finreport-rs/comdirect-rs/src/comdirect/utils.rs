use std::time::{SystemTime, UNIX_EPOCH};

pub fn request_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ts = now.as_millis().to_string();

    // Extract the last 9 characters.
    let len = ts.len();
    if len > 9 {
        ts[len - 9..].to_string()
    } else {
        ts
    }
}