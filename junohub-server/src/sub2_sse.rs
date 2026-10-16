use std::{io, pin::Pin};

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;

pub(crate) const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

pub(crate) enum Frame {
    Event(Value),
    Done,
}

pub(crate) fn frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    // Stop at the first boundary. Searching LF and CRLF separately rescans
    // the entire remaining body once per frame when only LF is present.
    for (index, pair) in buffer.windows(2).enumerate() {
        if pair == b"\n\n" {
            return Some((index, 2));
        }
        if pair == b"\r\n" && buffer.get(index + 2..index + 4) == Some(b"\r\n".as_slice()) {
            return Some((index, 4));
        }
    }
    None
}

pub(crate) fn decode(frame: &[u8]) -> io::Result<Option<Frame>> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("上游 SSE 单个事件超过 32MB 限制"));
    }
    let text =
        std::str::from_utf8(frame).map_err(|_| io::Error::other("上游 SSE 不是有效 UTF-8"))?;
    let data = text
        .trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let data = line.strip_prefix("data:")?;
            Some(data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() {
        return Ok(None);
    }
    if data == "[DONE]" {
        return Ok(Some(Frame::Done));
    }
    let event = serde_json::from_str::<Value>(data)
        .map_err(|_| io::Error::other("上游 SSE 事件不是有效 JSON"))?;
    if !event.is_object() {
        return Err(io::Error::other("上游 SSE 事件格式无效"));
    }
    Ok(Some(Frame::Event(event)))
}

pub(crate) fn events<S>(upstream: S) -> Pin<Box<dyn Stream<Item = io::Result<Value>> + Send>>
where
    S: Stream<Item = io::Result<Bytes>> + Send + 'static,
{
    Box::pin(async_stream::try_stream! {
        let mut upstream = Box::pin(upstream);
        let mut buffer = Vec::new();
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(|_| io::Error::other("上游流式响应读取中断"))?;
            // Bound memory even when a transport chunk contains many frames.
            for part in chunk.chunks(16 * 1024) {
                let mut scan_from = buffer.len().saturating_sub(3);
                buffer.extend_from_slice(part);
                while let Some((end, delimiter)) = frame_end(&buffer[scan_from..]) {
                    let end = scan_from + end;
                    let frame = decode(&buffer[..end])?;
                    buffer.drain(..end + delimiter);
                    scan_from = 0;
                    match frame {
                        Some(Frame::Event(event)) => yield event,
                        Some(Frame::Done) => return,
                        None => {}
                    }
                }
                if buffer.len() > MAX_FRAME_BYTES {
                    Err(io::Error::other("上游 SSE 单个事件超过 32MB 限制"))?;
                }
            }
        }
        if !buffer.iter().all(u8::is_ascii_whitespace) {
            Err(io::Error::other("上游 SSE 事件未完整结束"))?;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[test]
    fn first_frame_boundary_is_found_for_mixed_line_endings() {
        assert_eq!(frame_end(b"data: {}\n\ndata: {}\r\n\r\n"), Some((8, 2)));
        assert_eq!(frame_end(b"data: {}\r\n\r\ndata: {}\n\n"), Some((8, 4)));
        assert_eq!(frame_end(b"data: {}\r\n\r"), None);
    }

    #[tokio::test]
    async fn fragmented_multiline_crlf_and_utf8_are_preserved() {
        let body = "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\r\ndata: \"delta\":\"你好\"}\r\n\r\ndata: [DONE]\r\n\r\n";
        let input = stream::iter(
            body.as_bytes()
                .chunks(1)
                .map(|part| Ok(Bytes::copy_from_slice(part)))
                .collect::<Vec<_>>(),
        );
        let output = events(input).collect::<Vec<_>>().await;
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].as_ref().unwrap()["delta"], "你好");
    }

    #[tokio::test]
    async fn malformed_truncated_or_oversized_events_are_errors() {
        for body in [
            b"data: broken\n\n".to_vec(),
            b"data: {\"type\":\"ping\"}".to_vec(),
            vec![b'x'; MAX_FRAME_BYTES + 1],
        ] {
            let output = events(stream::iter(vec![Ok(Bytes::from(body))]))
                .collect::<Vec<_>>()
                .await;
            assert!(output.last().unwrap().is_err());
        }
        assert!(decode(&[0xff]).is_err());
    }
}
