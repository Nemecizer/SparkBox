//! A small Server-Sent Events reader for streamed model replies. Chunks can
//! split anywhere (even inside a UTF-8 character); events come out whole.

#[derive(Debug, Default, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(SseEvent { event: self.event.take(), data: self.data.join("\n") });
                    self.data.clear();
                }
                self.event = None;
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = Some(v.trim().to_string());
            }
            // ":" comments, "id:" and "retry:" lines are ignored.
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_survive_arbitrary_chunking() {
        let text = "event: message_start\ndata: {\"a\":1}\n\n: keepalive\n\ndata: héllo\r\n\r\ndata: [DONE]\n\n";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut p = SseParser::default();
            let mut got = p.push(&bytes[..split]);
            got.extend(p.push(&bytes[split..]));
            assert_eq!(
                got,
                vec![
                    SseEvent { event: Some("message_start".into()), data: "{\"a\":1}".into() },
                    SseEvent { event: None, data: "héllo".into() },
                    SseEvent { event: None, data: "[DONE]".into() },
                ],
                "split at {split}"
            );
        }
    }
}
