pub struct WdisMessage {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub fn parse_wdis_payload(data: &[u8]) -> Option<WdisMessage> {
    if data.len() < 20 || &data[0..4] != b"WDIS" {
        return None;
    }
    let start_ms = i64::from_le_bytes(data[4..12].try_into().ok()?);
    let end_ms = i64::from_le_bytes(data[12..20].try_into().ok()?);
    let text = String::from_utf8_lossy(&data[20..]).to_string();
    Some(WdisMessage { start_ms, end_ms, text })
}