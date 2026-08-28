use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub code: u16,
    pub message: &'static str,
    pub data: T,
}

impl<T> Envelope<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 200,
            message: "ok",
            data,
        }
    }
}
