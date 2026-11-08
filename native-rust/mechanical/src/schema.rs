use crate::{Error, Result};
use serde::Deserialize;
use serde_json::{Number, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Object,
    String,
    Boolean,
    Number,
    Integer,
    Array,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Array => "array",
        }
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Object => value.is_object(),
            Self::String => value.is_string(),
            Self::Boolean => value.is_boolean(),
            Self::Number => value.is_number(),
            // Preserve the bridge contract: 1.0 is a floating value, not an integer.
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Array => value.is_array(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Additional {
    Allowed(bool),
    Schema(Box<Schema>),
}

/// The exact vocabulary used by the embedded mechanical schema. New schema
/// keywords fail loading until their validation behavior is implemented.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schema {
    #[serde(rename = "type")]
    kind: Option<Kind>,
    #[serde(default)]
    properties: BTreeMap<String, Schema>,
    #[serde(default)]
    required: Vec<String>,
    additional_properties: Option<Additional>,
    one_of: Option<Vec<Schema>>,
    #[serde(rename = "enum")]
    variants: Option<Vec<Value>>,
    minimum: Option<Number>,
    maximum: Option<Number>,
    exclusive_minimum: Option<Number>,
    min_items: Option<usize>,
    max_items: Option<usize>,
    min_properties: Option<usize>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    items: Option<Box<Schema>>,
    #[serde(rename = "description")]
    _description: Option<String>,
    #[serde(rename = "default")]
    _default: Option<Value>,
}

impl Schema {
    pub(crate) fn has_property(&self, name: &str) -> bool {
        self.properties.contains_key(name)
    }

    fn discriminator_matches(&self, name: &str, value: &Value) -> bool {
        self.properties
            .get(name)
            .and_then(|entry| entry.variants.as_ref())
            .is_some_and(|variants| variants.contains(value))
    }

    pub fn validate(&self, value: &Value, path: &str) -> Result<()> {
        if let Some(variants) = &self.one_of {
            // Explain the selected primitive/action instead of unrelated variants.
            let selected = ["type", "action"].into_iter().find_map(|name| {
                let discriminator = value.as_object()?.get(name)?;
                let mut matching = variants
                    .iter()
                    .filter(|variant| variant.discriminator_matches(name, discriminator));
                let selected = matching.next()?;
                matching.next().is_none().then_some(selected)
            });
            if let Some(selected) = selected {
                selected.validate(value, path)?;
            } else {
                let count = variants
                    .iter()
                    .filter(|variant| variant.validate(value, path).is_ok())
                    .count();
                if count != 1 {
                    return Err(Error::input(format!(
                        "{path} must match exactly one supported schema variant"
                    )));
                }
            }
        }
        if let Some(kind) = self.kind {
            if !kind.accepts(value) {
                return Err(Error::input(format!("{path} must be {}", kind.name())));
            }
        }
        if matches!(self.kind, Some(Kind::Number | Kind::Integer)) {
            let number = value.as_number().expect("kind checked above");
            if !number.as_f64().is_some_and(f64::is_finite) {
                return Err(Error::input(format!("{path} must be finite")));
            }
            if self.exclusive_minimum.as_ref().is_some_and(|minimum| {
                compare_numbers(number, minimum) != std::cmp::Ordering::Greater
            }) || self
                .minimum
                .as_ref()
                .is_some_and(|minimum| compare_numbers(number, minimum) == std::cmp::Ordering::Less)
            {
                return Err(Error::input(format!("{path} is too small")));
            }
            if self.maximum.as_ref().is_some_and(|maximum| {
                compare_numbers(number, maximum) == std::cmp::Ordering::Greater
            }) {
                return Err(Error::input(format!("{path} is too large")));
            }
        }
        if matches!(self.kind, Some(Kind::Array)) {
            let array = value.as_array().expect("kind checked above");
            if array.len() < self.min_items.unwrap_or(0)
                || array.len() > self.max_items.unwrap_or(10_000)
            {
                return Err(Error::input(format!("{path} has an invalid item count")));
            }
            if let Some(schema) = &self.items {
                for (index, item) in array.iter().enumerate() {
                    schema.validate(item, &format!("{path}[{index}]"))?;
                }
            }
        }
        if matches!(self.kind, Some(Kind::String)) {
            let count = value.as_str().expect("kind checked above").chars().count();
            if count < self.min_length.unwrap_or(0)
                || self.max_length.is_some_and(|maximum| count > maximum)
            {
                return Err(Error::input(format!("{path} has an invalid string length")));
            }
        }
        if self
            .variants
            .as_ref()
            .is_some_and(|variants| !variants.contains(value))
        {
            return Err(Error::input(format!("{path} has an unsupported value")));
        }
        if matches!(self.kind, Some(Kind::Object)) {
            let object = value.as_object().expect("kind checked above");
            if object.len() < self.min_properties.unwrap_or(0) {
                return Err(Error::input(format!(
                    "{path} requires at least one property"
                )));
            }
            for key in &self.required {
                if !object.contains_key(key) {
                    return Err(Error::input(format!("{path}.{key} is required")));
                }
            }
            if matches!(self.additional_properties, Some(Additional::Allowed(false)))
                && object.keys().any(|key| !self.properties.contains_key(key))
            {
                return Err(Error::input(format!("{path} has unknown fields")));
            }
            // Sorted keys give deterministic first-error reporting.
            let fields: BTreeMap<_, _> = object.iter().collect();
            for (key, value) in fields {
                if let Some(schema) = self.properties.get(key) {
                    schema.validate(value, &format!("{path}.{key}"))?;
                } else if let Some(Additional::Schema(schema)) = &self.additional_properties {
                    schema.validate(value, &format!("{path}.{key}"))?;
                }
            }
        }
        Ok(())
    }
}

// Exact integer bounds matter for IDs and indexes beyond the 53-bit float range.
fn compare_numbers(left: &Number, right: &Number) -> std::cmp::Ordering {
    fn integer(number: &Number) -> Option<i128> {
        number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from))
    }
    match (integer(left), integer(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left
            .as_f64()
            .expect("finite JSON number")
            .partial_cmp(&right.as_f64().expect("finite JSON bound"))
            .expect("JSON numbers are finite"),
    }
}
