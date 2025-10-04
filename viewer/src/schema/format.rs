use itertools::Itertools;
use jsonschema::{
    BasicOutput,
    output::{ErrorDescription, OutputUnit},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::LazyLock,
};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub name: String,
    #[serde(skip_serializing_if = "is_default")]
    pub display_field: Option<String>,
    pub fields: Vec<Field>,
    #[serde(skip_serializing_if = "is_default")]
    pub relations: Option<HashMap<String, Vec<String>>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    #[serde(skip_serializing_if = "is_default")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "is_default")]
    pub r#type: FieldType,
    #[serde(skip_serializing_if = "is_default")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "is_default")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub fields: Option<Vec<Field>>,
    #[serde(skip_serializing_if = "is_default")]
    pub relations: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "is_default")]
    pub condition: Option<Condition>,
    #[serde(skip_serializing_if = "is_default")]
    pub targets: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    #[default]
    Scalar,
    Link,
    Array,
    Icon,
    ModelId,
    Color,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub switch: String,
    pub cases: HashMap<i32, Vec<String>>,
}

fn is_default<T: Default + Eq>(value: &T) -> bool {
    value == &T::default()
}

static SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    jsonschema::validator_for(
        &serde_json::from_str(include_str!("../../assets/schema.json")).unwrap(),
    )
    .unwrap()
});

impl Schema {
    pub fn from_str(
        s: &str,
    ) -> anyhow::Result<Result<Self, VecDeque<OutputUnit<ErrorDescription>>>> {
        let value: serde_json::Value = serde_yml::from_str(s)?;
        match SCHEMA.apply(&value).basic() {
            BasicOutput::Valid(_) => {
                let schema: Schema = serde_json::from_value(value)?;
                Ok(Ok(schema))
            }
            BasicOutput::Invalid(errors) => {
                for error in &errors {
                    log::error!(
                        "Schema Error: {} at path {}",
                        error.error_description(),
                        error.instance_location()
                    );
                }
                Ok(Err(errors))
            }
        }
    }

    pub fn from_blank(name: impl Into<String>, column_count: usize) -> Self {
        Self {
            name: name.into(),
            fields: (0..column_count)
                .map(|i| Field {
                    name: Some(format!("Unknown{i}")),
                    ..Default::default()
                })
                .collect_vec(),
            ..Default::default()
        }
    }

    pub fn misc_sheet(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: vec![
                Field {
                    name: Some("Key".to_string()),
                    r#type: FieldType::Scalar,
                    ..Default::default()
                },
                Field {
                    name: Some("Value".to_string()),
                    r#type: FieldType::Scalar,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }
}
