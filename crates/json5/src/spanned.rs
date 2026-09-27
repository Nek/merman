//! Opt-in original UTF-8 token bounds from the owning JSON5 deserializer.
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData, ops::Range};

pub(crate) const NAME: &str = "$json5::spanned";

/// A parsed value and its authored token, excluding surrounding whitespace/comments.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
}

/// Wraps an existing seed without changing how it parses the value.
pub struct SpannedSeed<S>(pub S);

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for SpannedSeed<S> {
    type Value = Spanned<S::Value>;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_newtype_struct(NAME, self)
    }
}

impl<'de, S: DeserializeSeed<'de>> Visitor<'de> for SpannedSeed<S> {
    type Value = Spanned<S::Value>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON5 value with its original token bounds")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let value = sequence
            .next_element_seed(self.0)?
            .ok_or_else(|| de::Error::custom("missing spanned value"))?;
        let start = sequence
            .next_element()?
            .ok_or_else(|| de::Error::custom("missing token start"))?;
        let end = sequence
            .next_element()?
            .ok_or_else(|| de::Error::custom("missing token end"))?;
        Ok(Spanned {
            value,
            span: start..end,
        })
    }
}

struct ValueSeed<T>(PhantomData<T>);
impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ValueSeed<T> {
    type Value = T;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        T::deserialize(deserializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Spanned<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        SpannedSeed(ValueSeed(PhantomData)).deserialize(deserializer)
    }
}
