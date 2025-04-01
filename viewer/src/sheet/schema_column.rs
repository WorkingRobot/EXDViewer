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
        scope: String,
        fields: &[Field],
        pending_names: bool,
        is_array: bool,
    ) -> anyhow::Result<()> {
        let column_offset = ret.len() as u32;

        let mut column_placeholder = u32::MAX;
        let mut column_lookups = vec![];

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
                            column_placeholder -= 1;
                            column_lookups.push(&condition.switch);
                            SchemaColumnMeta::ConditionalLink {
                                column_idx: column_placeholder,
                                links: condition.cases.clone(),
                            }
                        } else {
                            bail!("Link field missing targets or condition: {:?}", field);
                        }
                    }
                    FieldType::Array => unreachable!(),
                };

                ret.push(Self { name, meta });
            }
        }

        for i in 0..ret.len() - column_offset as usize {
            let column = &ret[column_offset as usize + i];
            if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = column.meta {
                let column_lookup_idx = u32::MAX - column_idx - 1;
                let column_lookup_name = match column_lookups.get(column_lookup_idx as usize) {
                    Some(&name) => name,
                    None => {
                        bail!(
                            "Failed to find column lookup name for {}'s conditional link: {}",
                            column.name,
                            column_lookup_idx
                        );
                    }
                };

                let resolved_column_idx = column_offset
                    + match ret[column_offset as usize..]
                        .iter()
                        .enumerate()
                        .find_map(|(i, c)| {
                            if c.name[scope.len()..] == *column_lookup_name {
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
                                column_lookup_name
                            );
                        }
                    };

                if let SchemaColumnMeta::ConditionalLink { column_idx, .. } =
                    &mut ret[column_offset as usize + i].meta
                {
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
        Self::get_columns_inner(&mut ret, "".to_string(), fields, pending_names, false)?;

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
