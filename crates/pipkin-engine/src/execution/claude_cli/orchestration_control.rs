//! Private authenticated transport control, never MCP output or native execution authority.
//! The helper rejects this reserved field on its MCP stdin; only the host can request close.
use serde_json::{Value, json};
const FIELD: &str = "pipkinNativeBridgeControl";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Quiesce,
    Quiesced,
    InputClosed,
    Closed,
}
impl Control {
    pub(crate) fn value(self) -> Value {
        json!({(FIELD):match self {Self::Quiesce=>"quiesce-v1",Self::Quiesced=>"quiesced-v1",Self::InputClosed=>"input-closed-v1",Self::Closed=>"closed-v1"}})
    }
    pub(crate) fn matches(self, value: &Value) -> bool {
        self.value() == *value
    }
}
pub(crate) fn reserved(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.contains_key(FIELD))
}
