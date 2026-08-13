use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequiredNullable<T> {
    Null,
    Value(T),
}

impl<T> RequiredNullable<T> {
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub const fn as_ref(&self) -> RequiredNullable<&T> {
        match self {
            Self::Null => RequiredNullable::Null,
            Self::Value(value) => RequiredNullable::Value(value),
        }
    }

    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Null => None,
            Self::Value(value) => Some(value),
        }
    }
}

impl<T> Serialize for RequiredNullable<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T> Deserialize<'de> for RequiredNullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        NullableValue::<T>::deserialize(deserializer).map(|value| match value {
            NullableValue::Null(()) => Self::Null,
            NullableValue::Value(value) => Self::Value(value),
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NullableValue<T> {
    Null(()),
    Value(T),
}

pub fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
