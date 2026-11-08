//! The input bundle every check takes: `{board, stack?, pours?, rules?, silk?, fab?, bodies?, sheetParts?, netlist?, args?}`, each the
//! standard model a bridge answers (schema/board.schema.json, schema/companions.schema.json). A bridge's whole reply
//! is accepted too: the model's keys at the top level, or under `data` (the reply envelope keys are ignored).
use serde_json::{json, Value};

/// A refusal in the standard reply envelope.
pub fn refuse(code: &str, error: &str) -> Value {
    json!({"success": false, "errorCode": code, "error": error})
}

/// The companion under `key`, unwrapped from a reply's `data` when it came as one.
pub fn part(input: &Value, key: &str) -> Option<Value> {
    let v = input.get(key)?;
    if v.is_null() { return None; }
    Some(if v.get("data").map(|d| d.is_object()).unwrap_or(false) { v["data"].clone() } else { v.clone() })
}

/// The board model, or `bad_args` when there is none or the bridge's reply was a refusal.
pub fn board(input: &Value) -> Result<Value, Value> {
    let Some(b) = part(input, "board") else { return Err(refuse("bad_args", "the input has no board: pass the board model (routing_state) as board")) };
    if b.get("success") == Some(&json!(false)) { return Err(b); }
    Ok(b)
}

/// The stack-up companion (layer_stack), if given.
pub fn stack(input: &Value) -> Option<Value> { part(input, "stack") }

/// The poured copper (pour_state's `zones`), empty when not given.
pub fn pours(input: &Value) -> Vec<Value> {
    let Some(p) = part(input, "pours") else { return Vec::new() };
    p.get("zones").or(Some(&p)).and_then(|z| z.as_array()).cloned().unwrap_or_default()
}

/// The board's rules (read_rules): a list, or a reply with `rules`.
pub fn rules(input: &Value) -> Vec<Value> {
    let Some(r) = part(input, "rules") else { return Vec::new() };
    r.get("rules").or(Some(&r)).and_then(|x| x.as_array()).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companions_come_bare_or_in_a_reply() {
        let bare = json!({"board": {"outline": []}, "stack": {"layers": [{"name": "Top"}]}, "pours": {"zones": [{"net": "GND"}]}});
        let wrapped = json!({"board": {"success": true, "outline": []}, "stack": {"success": true, "data": {"layers": [{"name": "Top"}]}}, "pours": [{"net": "GND"}]});
        for i in [bare, wrapped] {
            assert!(board(&i).is_ok());
            assert_eq!(stack(&i).unwrap()["layers"][0]["name"], "Top");
            assert_eq!(pours(&i)[0]["net"], "GND");
        }
        assert_eq!(board(&json!({})).unwrap_err()["errorCode"], "bad_args");
    }
}
