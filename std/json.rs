fn serde_to_oneway(v: serde_json::Value) -> JsonValue {
    match v {
        serde_json::Value::Null => JsonValue::JsonNull(()),
        serde_json::Value::Bool(b) => JsonValue::JsonBool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                JsonValue::JsonNumber(JsonNumber::JsonInt(i))
            } else {
                JsonValue::JsonNumber(JsonNumber::JsonFloat(n.as_f64().unwrap_or(f64::NAN)))
            }
        }
        serde_json::Value::String(s) => JsonValue::JsonString(s),
        serde_json::Value::Array(arr) => JsonValue::JsonArray(Box::new(JsonArray(
            arr.into_iter().map(serde_to_oneway).collect(),
        ))),
        serde_json::Value::Object(obj) => JsonValue::JsonObject(Box::new(JsonObject(
            obj.into_iter()
                .map(|(k, v)| JsonEntry {
                    jsonKey: JsonKey(k),
                    jsonValue: serde_to_oneway(v),
                })
                .collect(),
        ))),
    }
}

fn oneway_to_serde(v: JsonValue) -> serde_json::Value {
    match v {
        JsonValue::JsonNull(_) => serde_json::Value::Null,
        JsonValue::JsonBool(b) => serde_json::Value::Bool(b),
        JsonValue::JsonNumber(n) => match n {
            JsonNumber::JsonInt(i) => serde_json::Value::Number(i.into()),
            JsonNumber::JsonFloat(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        },
        JsonValue::JsonString(s) => serde_json::Value::String(s),
        JsonValue::JsonArray(arr) => {
            serde_json::Value::Array(arr.0.into_iter().map(oneway_to_serde).collect())
        }
        JsonValue::JsonObject(obj) => serde_json::Value::Object(
            obj.0
                .into_iter()
                .map(|e| (e.jsonKey.0, oneway_to_serde(e.jsonValue)))
                .collect(),
        ),
    }
}

#[allow(dead_code)]
fn oneway_json_parse(s: String) -> Result<JsonValue, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(&s)?;
    Ok(serde_to_oneway(v))
}

#[allow(dead_code)]
fn oneway_json_emit(v: JsonValue) -> String {
    oneway_to_serde(v).to_string()
}

#[allow(dead_code)]
fn oneway_bool_to_json(b: bool) -> JsonValue {
    JsonValue::JsonBool(b)
}

#[allow(dead_code)]
fn oneway_float_to_json(f: f64) -> JsonValue {
    JsonValue::JsonNumber(JsonNumber::JsonFloat(f))
}

#[allow(dead_code)]
fn oneway_int_to_json(i: i64) -> JsonValue {
    JsonValue::JsonNumber(JsonNumber::JsonInt(i))
}

#[allow(dead_code)]
fn oneway_string_to_json(s: String) -> JsonValue {
    JsonValue::JsonString(s)
}

impl JsonValue {
    #[allow(dead_code, non_snake_case)]
    fn asArray(self) -> Result<JsonArray, serde_json::Error> {
        match self {
            JsonValue::JsonArray(arr) => Ok(*arr),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected array",
            )),
        }
    }

    #[allow(dead_code, non_snake_case)]
    fn asBool(self) -> Result<bool, serde_json::Error> {
        match self {
            JsonValue::JsonBool(b) => Ok(b),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected boolean",
            )),
        }
    }

    #[allow(dead_code, non_snake_case)]
    fn asNull(self) -> Result<(), serde_json::Error> {
        match self {
            JsonValue::JsonNull(n) => Ok(n),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected null",
            )),
        }
    }

    #[allow(dead_code, non_snake_case)]
    fn asNumber(self) -> Result<JsonNumber, serde_json::Error> {
        match self {
            JsonValue::JsonNumber(n) => Ok(n),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected number",
            )),
        }
    }

    #[allow(dead_code, non_snake_case)]
    fn asObject(self) -> Result<JsonObject, serde_json::Error> {
        match self {
            JsonValue::JsonObject(obj) => Ok(*obj),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected object",
            )),
        }
    }

    #[allow(dead_code, non_snake_case)]
    fn asString(self) -> Result<String, serde_json::Error> {
        match self {
            JsonValue::JsonString(s) => Ok(s),
            _ => Err(<serde_json::Error as serde::de::Error>::custom(
                "expected string",
            )),
        }
    }
}

impl JsonArray {
    #[allow(dead_code)]
    fn items(self) -> Vec<JsonValue> {
        self.0
    }

    #[allow(dead_code)]
    fn length(&self) -> i64 {
        self.0.len() as i64
    }
}

impl JsonObject {
    #[allow(dead_code)]
    fn get(&self, key: String) -> Result<JsonValue, serde_json::Error> {
        self.0
            .iter()
            .find(|e| e.jsonKey.0 == key)
            .map(|e| e.jsonValue.clone())
            .ok_or_else(|| {
                <serde_json::Error as serde::de::Error>::custom(format!("missing key: {}", key))
            })
    }

    #[allow(dead_code)]
    fn length(&self) -> i64 {
        self.0.len() as i64
    }
}
