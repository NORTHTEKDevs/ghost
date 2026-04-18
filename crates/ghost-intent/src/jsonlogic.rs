//! Minimal JSONLogic evaluator. Subset needed for abort_if / retry_if:
//! ==, !=, <, <=, >, >=, &&, ||, !, in, var, contains.

use crate::error::IntentError;
use serde_json::{json, Value};

pub fn eval(expr: &Value, state: &Value) -> Result<Value, IntentError> {
    let obj = match expr {
        Value::Object(m) if m.len() == 1 => m,
        Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Null => {
            return Ok(expr.clone())
        }
        _ => return Err(IntentError::Invalid("expected operator object".into())),
    };
    let (op, args) = obj.iter().next().unwrap();
    match op.as_str() {
        "var" => {
            let path = match args {
                Value::String(s) => s.as_str(),
                Value::Array(a) if !a.is_empty() => a[0].as_str().unwrap_or(""),
                _ => return Err(IntentError::Invalid("var expects string".into())),
            };
            Ok(lookup_var(state, path))
        }
        "==" => cmp2(args, state, |a, b| Ok(json!(loose_eq(a, b)))),
        "!=" => cmp2(args, state, |a, b| Ok(json!(!loose_eq(a, b)))),
        "<" => cmp2(args, state, |a, b| Ok(json!(to_f64(&a) < to_f64(&b)))),
        "<=" => cmp2(args, state, |a, b| Ok(json!(to_f64(&a) <= to_f64(&b)))),
        ">" => cmp2(args, state, |a, b| Ok(json!(to_f64(&a) > to_f64(&b)))),
        ">=" => cmp2(args, state, |a, b| Ok(json!(to_f64(&a) >= to_f64(&b)))),
        "&&" => {
            let arr = args.as_array().ok_or_else(|| IntentError::Invalid("&& expects array".into()))?;
            for e in arr {
                let v = eval(e, state)?;
                if !truthy(&v) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        "||" => {
            let arr = args.as_array().ok_or_else(|| IntentError::Invalid("|| expects array".into()))?;
            for e in arr {
                let v = eval(e, state)?;
                if truthy(&v) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        "!" => {
            let arr = args.as_array().ok_or_else(|| IntentError::Invalid("! expects array".into()))?;
            let v = eval(arr.first().unwrap_or(&Value::Null), state)?;
            Ok(Value::Bool(!truthy(&v)))
        }
        "in" => {
            let arr = args.as_array().ok_or_else(|| IntentError::Invalid("in expects array".into()))?;
            if arr.len() != 2 {
                return Err(IntentError::Invalid("in expects 2 args".into()));
            }
            let needle = eval(&arr[0], state)?;
            let haystack = eval(&arr[1], state)?;
            let found = match &haystack {
                Value::Array(a) => a.iter().any(|v| loose_eq(v.clone(), needle.clone())),
                Value::String(s) => needle.as_str().map(|n| s.contains(n)).unwrap_or(false),
                _ => false,
            };
            Ok(Value::Bool(found))
        }
        "contains" => {
            let arr = args.as_array().ok_or_else(|| IntentError::Invalid("contains expects array".into()))?;
            if arr.len() != 2 {
                return Err(IntentError::Invalid("contains expects 2 args".into()));
            }
            let haystack = eval(&arr[0], state)?;
            let needle = eval(&arr[1], state)?;
            let found = haystack
                .as_str()
                .and_then(|h| needle.as_str().map(|n| h.contains(n)))
                .unwrap_or(false);
            Ok(Value::Bool(found))
        }
        other => Err(IntentError::Invalid(format!("unknown operator: {other}"))),
    }
}

/// Validate expression structure at compile time.
pub fn validate(expr: &Value) -> Result<(), IntentError> {
    match expr {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
        Value::Object(m) => {
            if m.len() != 1 {
                return Err(IntentError::Invalid("operator object must have one key".into()));
            }
            let (op, args) = m.iter().next().unwrap();
            match op.as_str() {
                "var" => Ok(()),
                "==" | "!=" | "<" | "<=" | ">" | ">=" | "in" | "contains" => {
                    let arr = args.as_array().ok_or_else(|| IntentError::Invalid(format!("{op} expects array")))?;
                    if arr.len() != 2 {
                        return Err(IntentError::Invalid(format!("{op} expects 2 args")));
                    }
                    for a in arr { validate(a)?; }
                    Ok(())
                }
                "&&" | "||" => {
                    let arr = args.as_array().ok_or_else(|| IntentError::Invalid(format!("{op} expects array")))?;
                    for a in arr { validate(a)?; }
                    Ok(())
                }
                "!" => {
                    let arr = args.as_array().ok_or_else(|| IntentError::Invalid("! expects array".into()))?;
                    if arr.len() != 1 { return Err(IntentError::Invalid("! expects 1 arg".into())); }
                    validate(&arr[0])
                }
                other => Err(IntentError::Invalid(format!("unknown operator: {other}"))),
            }
        }
        Value::Array(_) => Err(IntentError::Invalid("bare arrays not allowed".into())),
    }
}

fn cmp2<F>(args: &Value, state: &Value, f: F) -> Result<Value, IntentError>
where
    F: FnOnce(Value, Value) -> Result<Value, IntentError>,
{
    let arr = args.as_array().ok_or_else(|| IntentError::Invalid("expects array".into()))?;
    if arr.len() != 2 {
        return Err(IntentError::Invalid("binary op expects 2 args".into()));
    }
    let a = eval(&arr[0], state)?;
    let b = eval(&arr[1], state)?;
    f(a, b)
}

fn loose_eq(a: Value, b: Value) -> bool {
    match (&a, &b) {
        (Value::Number(x), Value::Number(y)) => to_f64(&a) == to_f64(&b) && x.is_i64() == y.is_i64() || to_f64(&a) == to_f64(&b),
        _ => a == b,
    }
}

fn to_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn lookup_var(state: &Value, path: &str) -> Value {
    if path.is_empty() {
        return state.clone();
    }
    let mut cur = state;
    for seg in path.split('.') {
        match cur {
            Value::Object(m) => {
                if let Some(v) = m.get(seg) { cur = v } else { return Value::Null }
            }
            _ => return Value::Null,
        }
    }
    cur.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equals_numbers() {
        assert!(eval(&json!({"==": [1, 1]}), &Value::Null).unwrap().as_bool().unwrap());
        assert!(!eval(&json!({"==": [1, 2]}), &Value::Null).unwrap().as_bool().unwrap());
    }

    #[test]
    fn var_reads_state() {
        let state = json!({"last_error": "timeout"});
        let r = eval(&json!({"==": [{"var": "last_error"}, "timeout"]}), &state).unwrap();
        assert!(r.as_bool().unwrap());
    }

    #[test]
    fn and_short_circuits() {
        let r = eval(&json!({"&&": [true, false, true]}), &Value::Null).unwrap();
        assert!(!r.as_bool().unwrap());
        let r = eval(&json!({"&&": [true, true]}), &Value::Null).unwrap();
        assert!(r.as_bool().unwrap());
    }

    #[test]
    fn contains_substring() {
        let r = eval(&json!({"contains": ["hello world", "world"]}), &Value::Null).unwrap();
        assert!(r.as_bool().unwrap());
        let r = eval(&json!({"contains": ["hello", "xyz"]}), &Value::Null).unwrap();
        assert!(!r.as_bool().unwrap());
    }

    #[test]
    fn validate_rejects_wrong_arity() {
        assert!(validate(&json!({"==": [1]})).is_err());
        assert!(validate(&json!({"!": [1, 2]})).is_err());
    }

    #[test]
    fn validate_rejects_unknown_op() {
        assert!(validate(&json!({"??": [1, 2]})).is_err());
    }

    #[test]
    fn validate_accepts_nested() {
        assert!(validate(&json!({"&&": [{"==": [1, 1]}, {"!=": [2, 3]}]})).is_ok());
    }
}
