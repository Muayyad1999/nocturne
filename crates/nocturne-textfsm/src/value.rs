//! Parsed output.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// One output row: value name (as written in the template, usually UPPERCASE) → value.
///
/// Every row of a template has the same keys, in the order of
/// [`Template::header`](crate::Template::header).
pub type Record = IndexMap<String, Value>;

/// The content of one cell.
///
/// A value declared with the `List` option yields [`Value::List`]; every other
/// value yields [`Value::Text`], the empty string if nothing was captured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    /// The text captured for a plain value.
    Text(String),
    /// The matches collected for a `List` value since the previous record.
    List(Vec<String>),
}

impl Value {
    /// The text of a [`Value::Text`]; `""` for a list.
    pub fn as_str(&self) -> &str {
        match self {
            Value::Text(text) => text,
            Value::List(_) => "",
        }
    }

    /// The items of a [`Value::List`]; an empty slice for text.
    pub fn as_list(&self) -> &[String] {
        match self {
            Value::Text(_) => &[],
            Value::List(items) => items,
        }
    }

    /// Whether this is the empty string or a list without items.
    pub fn is_empty(&self) -> bool {
        match self {
            Value::Text(text) => text.is_empty(),
            Value::List(items) => items.is_empty(),
        }
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Value {
        Value::Text(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value::Text(text)
    }
}

impl From<Vec<String>> for Value {
    fn from(items: Vec<String>) -> Value {
        Value::List(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors() {
        let text = Value::from("up");
        assert_eq!((text.as_str(), text.as_list().len(), text.is_empty()), ("up", 0, false));
        assert!(Value::from("").is_empty());

        let list = Value::from(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!((list.as_str(), list.as_list(), list.is_empty()), ("", &["a".to_owned(), "b".to_owned()][..], false));
        assert!(Value::List(Vec::new()).is_empty());
        assert!(!Value::List(vec![String::new()]).is_empty());
    }

    #[test]
    fn serializes_untagged() {
        let record: Record = [
            ("NAME".to_owned(), Value::from("eth0")),
            ("ADDRESSES".to_owned(), Value::from(vec!["10.0.0.1".to_owned()])),
        ]
        .into_iter()
        .collect();
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(json, r#"{"NAME":"eth0","ADDRESSES":["10.0.0.1"]}"#);
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
    }
}
