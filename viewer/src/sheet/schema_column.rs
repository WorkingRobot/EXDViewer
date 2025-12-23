use std::{cell::OnceCell, collections::HashMap, rc::Rc};

use anyhow::bail;
use itertools::Itertools;

use crate::{
    excel::provider::ExcelSheet,
    schema::{Field, FieldType, Schema},
    sheet::{GlobalContext, TableContext, table_context::SharedConvertibleSheetPromise},
};

#[derive(Debug, Clone)]
pub struct SchemaColumn(Rc<SchemaColumnImpl>);

#[derive(Debug)]
struct SchemaColumnImpl {
    name: String,
    meta: SchemaColumnMeta,
    comment: Option<String>,
}

impl SchemaColumn {
    pub fn name(&self) -> &str {
        &self.0.name
    }

    pub fn meta(&self) -> &SchemaColumnMeta {
        &self.0.meta
    }

    pub fn comment(&self) -> Option<&str> {
        self.0.comment.as_deref()
    }

    fn get_columns_inner(
        ret: &mut Vec<Self>,
        column_placeholder: &mut u32,
        column_lookups: &mut Vec<String>,
        scope: String,
        fields: &[Field],
        is_array: bool,
    ) -> anyhow::Result<()> {
        for field in fields {
            let mut scope = scope.clone();
            if is_array {
                if let Some(name) = &field.name {
                    scope.push('.');
                    scope.push_str(name);
                }
            } else {
                scope.push_str(field.name.as_deref().unwrap_or("Unk"));
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
                        scope.clone() + &format!("[{i}]"),
                        subfields,
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
                            SchemaColumnMeta::Link(SheetLink::new(targets.clone()))
                        } else if let Some(condition) = &field.condition {
                            column_lookups.push(condition.switch.clone());
                            let ret = SchemaColumnMeta::ConditionalLink {
                                column_idx: *column_placeholder,
                                links: condition
                                    .cases
                                    .iter()
                                    .map(|(k, v)| (*k, SheetLink::new(v.clone())))
                                    .collect(),
                            };
                            *column_placeholder += 1;
                            ret
                        } else {
                            bail!("Link field missing targets or condition: {field:?}");
                        }
                    }
                    FieldType::Array => unreachable!(),
                };

                ret.push(Self::new(name, meta, field.comment.clone()));
            }
        }

        Ok(())
    }

    fn resolve_placeholders(ret: &mut [Self], column_lookups: &[String]) -> anyhow::Result<()> {
        for i in 0..ret.len() {
            let column = &ret[i];
            if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = column.meta() {
                let Some(switch_name) = column_lookups.get(*column_idx as usize) else {
                    bail!(
                        "Failed to find column lookup name for {}'s conditional link: {}",
                        column.name(),
                        *column_idx
                    );
                };

                let Some(resolved_column_idx) = ret.iter().enumerate().find_map(|(i, c)| {
                    if c.name() == *switch_name {
                        Some(i as u32)
                    } else {
                        None
                    }
                }) else {
                    bail!(
                        "Failed to find column index for {}'s conditional link: {}",
                        column.name(),
                        switch_name
                    );
                };

                if matches!(ret[i].meta(), SchemaColumnMeta::ConditionalLink { .. }) {
                    let name = ret[i].0.name.clone();
                    let mut meta = ret[i].0.meta.clone();
                    if let SchemaColumnMeta::ConditionalLink { column_idx, .. } = &mut meta {
                        *column_idx = resolved_column_idx;
                    } else {
                        unreachable!();
                    }
                    ret[i] = Self::new(name, meta, None);
                } else {
                    unreachable!();
                }
            }
        }
        Ok(())
    }

    pub fn from_schema(schema: &Schema) -> anyhow::Result<(Vec<Self>, Option<u32>)> {
        let fields = &schema.fields;

        let mut ret = vec![];
        let mut column_placeholder = 0;
        let mut column_lookups = vec![];
        Self::get_columns_inner(
            &mut ret,
            &mut column_placeholder,
            &mut column_lookups,
            String::new(),
            fields,
            false,
        )?;
        Self::resolve_placeholders(&mut ret, &column_lookups)?;

        let display_idx = if let Some(display_field) = &schema.display_field {
            ret.iter()
                .find_position(|c| c.name() == *display_field)
                .map(|f| f.0 as u32)
        } else {
            None
        };

        Ok((ret, display_idx))
    }

    pub fn from_blank(column_count: usize) -> Vec<Self> {
        Self::from_schema(&Schema::from_blank("Blank", column_count))
            .unwrap()
            .0
    }

    pub fn new(name: String, meta: SchemaColumnMeta, comment: Option<String>) -> Self {
        Self(Rc::new(SchemaColumnImpl {
            name,
            meta,
            comment,
        }))
    }
}

#[derive(Debug, Clone)]
pub enum SchemaColumnMeta {
    Scalar,
    Icon,
    ModelId,
    Color,
    Link(Rc<SheetLink>),
    ConditionalLink {
        column_idx: u32,
        links: HashMap<i32, Rc<SheetLink>>,
    },
}

pub struct SheetLink {
    targets: Vec<String>,
    promises: OnceCell<Vec<SharedConvertibleSheetPromise>>,
}

impl SheetLink {
    pub fn new(targets: Vec<String>) -> Rc<Self> {
        Rc::new(Self {
            targets,
            promises: OnceCell::new(),
        })
    }

    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    pub fn resolve(
        &self,
        table: &TableContext,
        row_id: u32,
    ) -> Option<Option<(&String, TableContext)>> {
        self.resolve_internal(|| table.load_sheets(&self.targets), table.global(), row_id)
    }

    fn resolve_internal(
        &self,
        promise_initializer: impl Fn() -> Vec<SharedConvertibleSheetPromise>,
        global: &GlobalContext,
        row_id: u32,
    ) -> Option<Option<(&String, TableContext)>> {
        let promises = self.promises.get_or_init(promise_initializer);
        promises.iter().zip(self.targets.iter()).find_map(|(p, s)| {
            let mut p = p.borrow_mut();
            let result = p.get(|result| {
                result
                    .map(|(sheet, schema)| {
                        TableContext::new(global.clone(), sheet, schema.as_ref())
                    })
                    .map_err(|e| e.into())
            });
            match result {
                None => Some(None),
                Some(Ok(table)) => {
                    if table.sheet().get_row(row_id).is_ok() {
                        Some(Some((s, table.clone())))
                    } else {
                        None
                    }
                }
                Some(Err(err)) => {
                    log::error!("Failed to retrieve linked sheet: {err:?}");
                    None
                }
            }
        })
    }
}

impl std::fmt::Debug for SheetLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetLink")
            .field("targets", &self.targets)
            .finish_non_exhaustive()
    }
}
