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
    pub switch: Option<String>,
    pub cases: Option<HashMap<i32, Vec<String>>>,
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

    fn get_paths_inner(
        ret: &mut Vec<String>,
        scope: String,
        fields: &[Field],
        pending_names: bool,
        is_array: bool,
    ) {
        for field in fields {
            let mut scope = scope.clone();
            if is_array {
                if let Some(name) = field.name(pending_names) {
                    scope.push('.');
                    scope.push_str(name);
                }
            } else {
                scope.push_str(field.name(pending_names).unwrap_or("Unk"));
            }
            if field.r#type == FieldType::Array {
                let subfields = field.fields.as_deref();
                let subfields = match subfields {
                    Some(subfields) => subfields,
                    None => &[Field::default()],
                };
                for i in 0..(field.count.unwrap_or(1)) {
                    Self::get_paths_inner(
                        ret,
                        scope.clone() + &format!("[{}]", i),
                        subfields,
                        pending_names,
                        true,
                    );
                }
            } else {
                ret.push(scope);
            }
        }
    }

    pub fn get_paths(&self, pending_fields: bool, pending_names: bool) -> Vec<String> {
        let fields = if pending_fields {
            self.pending_fields.as_ref().unwrap_or(&self.fields)
        } else {
            &self.fields
        };
        let mut ret = vec![];
        Self::get_paths_inner(&mut ret, "".to_string(), fields, pending_names, false);
        ret
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
