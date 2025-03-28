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
    pub display_field: Option<String>,
    pub fields: Vec<Field>,
    pub pending_fields: Option<Vec<Field>>,
    pub relations: Option<HashMap<String, Vec<String>>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: Option<String>,
    pub pending_name: Option<String>,
    #[serde(default)]
    pub r#type: FieldType,
    pub count: Option<u32>,
    pub comment: Option<String>,
    pub fields: Option<Vec<Field>>,
    pub relations: Option<HashMap<String, Vec<String>>>,
    pub condition: Option<Condition>,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub switch: String,
    pub cases: HashMap<i32, Vec<String>>,
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
                    )
                }
                Ok(Err(errors))
            }
        }
    }
}

impl Field {
    pub fn name(&self, pending: bool) -> Option<&str> {
        if pending {
            self.pending_name.as_deref().or(self.name.as_deref())
        } else {
            self.name.as_deref()
        }
    }
}
