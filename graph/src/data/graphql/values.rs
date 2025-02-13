use failure::Error;
use graphql_parser::query::Value;
use std::collections::BTreeMap;
use std::str::FromStr;

use crate::prelude::format_err;
use web3::types::{H160, H256};

pub trait TryFromValue: Sized {
    fn try_from_value(value: &Value) -> Result<Self, Error>;
}

impl TryFromValue for Value {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        Ok(value.clone())
    }
}

impl TryFromValue for bool {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::Boolean(b) => Ok(*b),
            _ => Err(format_err!(
                "Cannot parse value into a boolean: {:?}",
                value
            )),
        }
    }
}

impl TryFromValue for String {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::String(s) => Ok(s.clone()),
            Value::Enum(s) => Ok(s.clone()),
            _ => Err(format_err!("Cannot parse value into a string: {:?}", value)),
        }
    }
}

impl TryFromValue for H160 {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::String(s) => {
                // `H160::from_str` takes a hex string with no leading `0x`.
                let string = s.trim_start_matches("0x");
                H160::from_str(string).map_err(|e| {
                    format_err!("Cannot parse Address/H160 value from string `{}`: {}", s, e)
                })
            }
            _ => Err(format_err!(
                "Cannot parse value into an Address/H160: {:?}",
                value
            )),
        }
    }
}

impl TryFromValue for H256 {
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::String(s) => {
                // `H256::from_str` takes a hex string with no leading `0x`.
                let string = s.trim_start_matches("0x");
                H256::from_str(string)
                    .map_err(|e| format_err!("Cannot parse H256 value from string `{}`: {}", s, e))
            }
            _ => Err(format_err!("Cannot parse value into an H256: {:?}", value)),
        }
    }
}

impl<T> TryFromValue for Vec<T>
where
    T: TryFromValue,
{
    fn try_from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::List(values) => values.into_iter().try_fold(vec![], |mut values, value| {
                values.push(T::try_from_value(value)?);
                Ok(values)
            }),
            _ => Err(format_err!("Cannot parse value into a vector: {:?}", value)),
        }
    }
}

pub trait ValueMap {
    fn get_required<T>(&self, key: &str) -> Result<T, Error>
    where
        T: TryFromValue;
    fn get_optional<T>(&self, key: &str) -> Result<Option<T>, Error>
    where
        T: TryFromValue;
}

impl ValueMap for Value {
    fn get_required<T>(&self, key: &str) -> Result<T, Error>
    where
        T: TryFromValue,
    {
        match self {
            Value::Object(map) => map.get_required(key),
            _ => Err(format_err!("value is not a map: {:?}", self)),
        }
    }

    fn get_optional<T>(&self, key: &str) -> Result<Option<T>, Error>
    where
        T: TryFromValue,
    {
        match self {
            Value::Object(map) => map.get_optional(key),
            _ => Err(format_err!("value is not a map: {:?}", self)),
        }
    }
}

impl ValueMap for &BTreeMap<String, Value> {
    fn get_required<T>(&self, key: &str) -> Result<T, Error>
    where
        T: TryFromValue,
    {
        self.get(key)
            .ok_or_else(|| format_err!("Required field `{}` not set", key))
            .and_then(|value| T::try_from_value(value).map_err(|e| e.into()))
    }

    fn get_optional<T>(&self, key: &str) -> Result<Option<T>, Error>
    where
        T: TryFromValue,
    {
        self.get(key).map_or(Ok(None), |value| {
            T::try_from_value(value)
                .map(|value| Some(value))
                .map_err(|e| e.into())
        })
    }
}

pub trait ValueList {
    fn get_values<T>(&self) -> Result<Vec<T>, Error>
    where
        T: TryFromValue;
}

impl ValueList for Value {
    fn get_values<T>(&self) -> Result<Vec<T>, Error>
    where
        T: TryFromValue,
    {
        match self {
            Value::List(values) => values.get_values(),
            _ => Err(format_err!("value is not a list: {:?}", self)),
        }
    }
}

impl ValueList for Vec<Value> {
    fn get_values<T>(&self) -> Result<Vec<T>, Error>
    where
        T: TryFromValue,
    {
        self.iter().try_fold(vec![], |mut acc, value| {
            acc.push(T::try_from_value(value)?);
            Ok(acc)
        })
    }
}
