use std::collections::HashMap;

use anyhow::bail;
use itertools::Itertools;

use crate::schema::{Field, FieldType, Schema};

#[derive(Debug, Clone)]
pub struct SchemaColumn {
    pub name: String,
    pub meta: SchemaColumnMeta,
}

impl SchemaColumn {
    fn get_columns_inner(
        ret: &mut Vec<Self>,
        column_placeholder: &mut u32,
        column_lookups: &mut Vec<String>,
        scope: String,
        fields: &[Field],
        pending_names: bool,
        is_array: bool,
    ) -> anyhow::Result<()> {
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
                    Self::get_columns_inner(
                        ret,
                        column_placeholder,
                        column_lookups,
                        scope.clone() + &format!("[{}]", i),
                        subfields,
                        pending_names,
                        true,
                    )?;
                }
            } else {
                let name = scope;

                let meta = match field.r#type {
                    FieldType::Scalar => SchemaColumnMeta::Scalar,
                    FieldType::Icon => SchemaColumnMeta::Icon,
                    FieldType::ModelId => SchemaColumnMeta::ModelId,
                    FieldType::Color => SchemaColumnMeta::Color,
                    FieldType::Link => {
                        if let Some(targets) = &field.targets {
                            SchemaColumnMeta::Link(targets.clone())
                        } else if let Some(condition) = &field.condition {
                            column_lookups.push(condition.switch.clone());
                            let ret = SchemaColumnMeta::ConditionalLink {
                                column_idx: *column_placeholder,
                                links: condition.cases.clone(),
                            };
                            *column_placeholder += 1;
                            ret
                        } else {
                            bail!("Link field missing targets or condition: {:?}", field);
                        }
                    }
                    FieldType::Array => unreachable!(),
                };

                ret.push(Self { name, meta });
            }
        }

        Ok(())
    }

    fn resolve_placeholders(
        ret: &mut Vec<Self>,
        column_lookups: &Vec<String>,
    ) -> anyhow::Result<()> {
        for i in 0..ret.len() {
            let column = &ret[i];
            if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = &column.meta {
                let switch_name = match column_lookups.get(*column_idx as usize) {
                    Some(name) => name,
                    None => {
                        bail!(
                            "Failed to find column lookup name for {}'s conditional link: {}",
                            column.name,
                            *column_idx
                        );
                    }
                };

                let resolved_column_idx = match ret.iter().enumerate().find_map(|(i, c)| {
                    if c.name == *switch_name {
                        Some(i as u32)
                    } else {
                        None
                    }
                }) {
                    Some(idx) => idx,
                    None => {
                        bail!(
                            "Failed to find column index for {}'s conditional link: {}",
                            column.name,
                            switch_name
                        );
                    }
                };

                if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = &mut ret[i].meta {
                    *column_idx = resolved_column_idx;
                } else {
                    unreachable!();
                }
            }
        }
        Ok(())
    }

    pub fn from_schema(
        schema: &Schema,
        pending_fields: bool,
        pending_names: bool,
    ) -> anyhow::Result<(Vec<Self>, Option<u32>)> {
        let fields = pending_fields
            .then_some(())
            .and(schema.pending_fields.as_ref())
            .unwrap_or(&schema.fields);

        let mut ret = vec![];
        let mut column_placeholder = 0;
        let mut column_lookups = vec![];
        Self::get_columns_inner(
            &mut ret,
            &mut column_placeholder,
            &mut column_lookups,
            "".to_string(),
            fields,
            pending_names,
            false,
        )?;
        Self::resolve_placeholders(&mut ret, &column_lookups)?;

        let display_idx = if let Some(display_field) = &schema.display_field {
            ret.iter()
                .find_position(|c| c.name == *display_field)
                .map(|f| f.0 as u32)
        } else {
            None
        };

        Ok((ret, display_idx))
    }

    pub fn from_blank(column_count: u32) -> Vec<Self> {
        (0..column_count)
            .map(|i| Self {
                name: format!("Column{}", i),
                meta: SchemaColumnMeta::Scalar,
            })
            .collect()
    }

    pub fn new(name: String, meta: SchemaColumnMeta) -> Self {
        Self { name, meta }
    }
}

#[derive(Debug, Clone)]
pub enum SchemaColumnMeta {
    Scalar,
    Icon,
    ModelId,
    Color,
    Link(Vec<String>),
    ConditionalLink {
        column_idx: u32,
        links: HashMap<i32, Vec<String>>,
    },
}
