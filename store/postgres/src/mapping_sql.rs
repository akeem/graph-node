///! This module contains the gory details of using Diesel to query
///! a database schema that is not known at compile time. The code in this
///! module is mostly concerned with constructing SQL queries and some
///! helpers for serializing and deserializing entities.
///!
///! Code in this module works very hard to minimize the number of allocations
///! that it performs
use diesel::pg::{Pg, PgConnection};
use diesel::query_builder::{AstPass, QueryFragment, QueryId};
use diesel::query_dsl::{LoadQuery, RunQueryDsl};
use diesel::result::QueryResult;
use diesel::sql_types::{Array, Binary, Bool, Integer, Jsonb, Nullable, Numeric, Text};
use diesel::Connection;
use failure::Fail;
use std::convert::TryFrom;
use std::str::FromStr;

use graph::data::store::scalar;
use graph::prelude::{
    format_err, serde_json, Attribute, Entity, EntityFilter, EntityKey, StoreError, Value,
};

use crate::filter::UnsupportedFilter;
use crate::mapping::{Column, ColumnType, Mapping, SqlName, Table};
use crate::sql_value::SqlValue;

/// Helper struct for retrieving entities from the database. With diesel, we
/// can only run queries that return columns whose number and type are known
/// at compile time. Because of that, we retrieve the actual data for an
/// entity as Jsonb by converting the row containing the entity using the
/// `to_jsonb` function.
#[derive(QueryableByName)]
pub struct EntityData {
    #[sql_type = "Text"]
    entity: String,
    #[sql_type = "Jsonb"]
    data: serde_json::Value,
}

impl EntityData {
    fn value_from_json(
        column_type: ColumnType,
        json: serde_json::Value,
    ) -> Result<graph::prelude::Value, StoreError> {
        use graph::prelude::Value as g;
        use serde_json::Value as j;
        // Many possible conversion errors are already caught by how
        // we define the schema; for example, we can only get a NULL for
        // a column that is actually nullable
        match json {
            j::Null => Ok(g::Null),
            j::Bool(b) => Ok(g::Bool(b)),
            j::Number(number) => match column_type {
                ColumnType::Int => match number.as_i64() {
                    Some(i) => i32::try_from(i).map(|i| g::Int(i)).map_err(|e| {
                        StoreError::Unknown(format_err!(
                            "failed to convert {} to Int: {}",
                            number,
                            e
                        ))
                    }),
                    None => Err(StoreError::Unknown(format_err!(
                        "failed to convert {} to Int",
                        number
                    ))),
                },
                ColumnType::BigDecimal => {
                    let s = number.to_string();
                    scalar::BigDecimal::from_str(s.as_str())
                        .map(|d| g::BigDecimal(d))
                        .map_err(|e| {
                            StoreError::Unknown(format_err!(
                                "failed to convert {} to BigDecimal: {}",
                                number,
                                e
                            ))
                        })
                }
                ColumnType::BigInt => {
                    let s = number.to_string();
                    scalar::BigInt::from_str(s.as_str())
                        .map(|d| g::BigInt(d))
                        .map_err(|e| {
                            StoreError::Unknown(format_err!(
                                "failed to convert {} to BigInt: {}",
                                number,
                                e
                            ))
                        })
                }
                column_type => Err(StoreError::Unknown(format_err!(
                    "can not convert number {} to {:?}",
                    number,
                    column_type
                ))),
            },
            j::String(s) => match column_type {
                ColumnType::String => Ok(g::String(s)),
                ColumnType::Bytes => scalar::Bytes::from_str(s.trim_start_matches("\\x"))
                    .map(|b| g::Bytes(b))
                    .map_err(|e| {
                        StoreError::Unknown(format_err!("failed to convert {} to Bytes: {}", s, e))
                    }),
                column_type => Err(StoreError::Unknown(format_err!(
                    "can not convert string {} to {:?}",
                    s,
                    column_type
                ))),
            },
            j::Array(values) => Ok(g::List(
                values
                    .into_iter()
                    .map(|v| Self::value_from_json(column_type, v))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            j::Object(_) => unimplemented!(),
        }
    }

    /// Map the `EntityData` to an entity using the schema information
    /// in `Mapping`
    pub fn to_entity(self, mapping: &Mapping) -> Result<Entity, StoreError> {
        let table = mapping.table_for_entity(&self.entity)?;

        use serde_json::Value as j;
        match self.data {
            j::Object(map) => {
                let mut entity = Entity::new();
                entity.insert(
                    "__typename".to_owned(),
                    graph::prelude::Value::from(self.entity),
                );
                for (key, json) in map {
                    let column = table.column(&SqlName::from(&*key))?;
                    let value = Self::value_from_json(column.column_type, json)?;
                    if value != Value::Null {
                        entity.insert(column.field.clone(), value);
                    }
                }
                Ok(entity)
            }
            _ => unreachable!(),
        }
    }
}

fn query_builder_error(msg: String) -> diesel::result::Error {
    diesel::result::Error::QueryBuilderError(Box::new(
        StoreError::Unknown(format_err!("{}", msg)).compat(),
    ))
}

/// A `QueryValue` makes it possible to bind a `Value` into a SQL query
/// where the needed SQL type is `ColumnType`
struct QueryValue<'a>(&'a Value, ColumnType);

impl<'a> QueryFragment<Pg> for QueryValue<'a> {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        match self.0 {
            Value::String(s) => out.push_bind_param::<Text, _>(s),
            Value::Int(i) => out.push_bind_param::<Integer, _>(i),
            Value::BigDecimal(d) => out.push_bind_param::<Numeric, _>(d),
            Value::Bool(b) => out.push_bind_param::<Bool, _>(b),
            Value::List(values) => {
                let values = SqlValue::new_array(values.clone());
                match self.1 {
                    ColumnType::BigDecimal | ColumnType::BigInt => {
                        out.push_bind_param::<Array<Numeric>, _>(&values)
                    }
                    ColumnType::Boolean => out.push_bind_param::<Array<Bool>, _>(&values),
                    ColumnType::Bytes => out.push_bind_param::<Array<Binary>, _>(&values),
                    ColumnType::Int => out.push_bind_param::<Array<Integer>, _>(&values),
                    ColumnType::String => out.push_bind_param::<Array<Text>, _>(&values),
                }
            }
            Value::Null => {
                let none: Option<&str> = None;
                out.push_bind_param::<Nullable<Text>, _>(&none)
            }
            Value::Bytes(b) => out.push_bind_param::<Binary, _>(&b.as_slice()),
            Value::BigInt(i) => {
                out.push_bind_param::<Numeric, _>(&i.clone().to_big_decimal(0.into()))
            }
        }
    }
}

/// A `QueryFilter` adds the conditions represented by the `filter` to
/// the `where` clause of a SQL query. The attributes mentioned in
/// the `filter` must all come from the given `table`, which is used to
/// map GraphQL names to column names, and to determine the type of the
/// column an attribute refers to
#[derive(Constructor)]
struct QueryFilter<'a> {
    filter: &'a EntityFilter,
    table: &'a Table,
}

impl<'a> QueryFilter<'a> {
    fn with(&self, filter: &'a EntityFilter) -> Self {
        QueryFilter {
            filter,
            table: self.table,
        }
    }

    fn column(&self, attribute: &Attribute) -> QueryResult<&'a Column> {
        self.table
            .column_for_field(attribute)
            .map_err(|e| query_builder_error(e.to_string()))
    }

    fn binary_op(
        &self,
        filters: &Vec<EntityFilter>,
        op: &str,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        out.push_sql(" (");
        for (i, filter) in filters.iter().enumerate() {
            if i > 0 {
                out.push_sql(op);
            }
            self.with(&filter).walk_ast(out.reborrow())?;
        }
        out.push_sql(") ");
        Ok(())
    }

    fn contains(
        &self,
        attribute: &Attribute,
        value: &Value,
        negated: bool,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        let column = self.column(attribute)?;

        match value {
            Value::String(s) => {
                out.push_identifier(column.name.as_str())?;
                if negated {
                    out.push_sql(" not like ");
                } else {
                    out.push_sql(" like ")
                };
                if s.starts_with('%') || s.ends_with('%') {
                    out.push_bind_param::<Text, _>(s)?;
                } else {
                    let s = format!("%{}%", s);
                    out.push_bind_param::<Text, _>(&s)?;
                }
            }
            Value::Bytes(b) => {
                out.push_sql("position(");
                out.push_bind_param::<Binary, _>(&b.as_slice())?;
                out.push_sql(" in ");
                out.push_identifier(column.name.as_str())?;
                if negated {
                    out.push_sql(") = 0")
                } else {
                    out.push_sql(") > 0");
                }
            }
            Value::List(_) => unimplemented!(
                "not clear from the JSONB implementation what that is supposed to do"
            ),
            Value::Null
            | Value::BigDecimal(_)
            | Value::Int(_)
            | Value::Bool(_)
            | Value::BigInt(_) => {
                let filter = match negated {
                    false => "contains",
                    true => "not_contains",
                };
                return Err(UnsupportedFilter {
                    filter: filter.to_owned(),
                    value: value.clone(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn equals(
        &self,
        attribute: &Attribute,
        value: &Value,
        negated: bool,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        let column = self.column(attribute)?;

        out.push_identifier(column.name.as_str())?;

        match value {
            Value::String(_)
            | Value::BigInt(_)
            | Value::Bool(_)
            | Value::Bytes(_)
            | Value::BigDecimal(_)
            | Value::Int(_)
            | Value::List(_) => {
                if negated {
                    out.push_sql(" != ");
                } else {
                    out.push_sql(" = ");
                };

                QueryValue(value, column.column_type).walk_ast(out)?;
            }
            Value::Null => {
                if negated {
                    out.push_sql(" is not null");
                } else {
                    out.push_sql(" is null");
                }
            }
        }
        Ok(())
    }

    fn compare(
        &self,
        attribute: &Attribute,
        value: &Value,
        op: &str,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        let column = self.column(attribute)?;

        out.push_identifier(column.name.as_str())?;
        out.push_sql(op);
        match value {
            Value::BigInt(_) | Value::BigDecimal(_) | Value::Int(_) | Value::String(_) => {
                QueryValue(value, column.column_type).walk_ast(out)?
            }
            Value::Bool(_) | Value::Bytes(_) | Value::List(_) | Value::Null => {
                return Err(UnsupportedFilter {
                    filter: op.to_owned(),
                    value: value.clone(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn in_array(
        &self,
        attribute: &Attribute,
        values: &Vec<Value>,
        negated: bool,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        let column = self.column(attribute)?;

        // NULLs in SQL are very special creatures, and we need to treat
        // them special. For non-NULL values, we generate
        //   attribute {in|not in} (value1, value2, ...)
        // and for NULL values we generate
        //   attribute {is|is not} null
        // If we have both NULL and non-NULL values we join these
        // two clauses with OR.
        //
        // Note that when we have no non-NULL values at all, we must
        // not generate `attribute {in|not in} ()` since the empty `()`
        // is a syntax error
        let have_nulls = values.iter().any(|value| value == &Value::Null);
        let have_non_nulls = values.iter().any(|value| value != &Value::Null);

        if have_nulls && have_non_nulls {
            out.push_sql("(");
        }

        if have_nulls {
            out.push_identifier(column.name.as_str())?;
            if negated {
                out.push_sql(" is not null");
            } else {
                out.push_sql(" is null")
            }
        }

        if have_nulls && have_non_nulls {
            out.push_sql(" or ");
        }

        if have_non_nulls {
            out.push_identifier(column.name.as_str())?;
            if negated {
                out.push_sql(" not in (");
            } else {
                out.push_sql(" in (");
            }
            for (i, value) in values
                .iter()
                .filter(|value| value != &&Value::Null)
                .enumerate()
            {
                if i > 0 {
                    out.push_sql(", ");
                }
                QueryValue(&value, column.column_type).walk_ast(out.reborrow())?;
            }
            out.push_sql(")");
        }

        if have_nulls && have_non_nulls {
            out.push_sql(")");
        }
        Ok(())
    }

    fn starts_or_ends_with(
        &self,
        attribute: &Attribute,
        value: &Value,
        op: &str,
        starts_with: bool,
        mut out: AstPass<Pg>,
    ) -> QueryResult<()> {
        let column = self.column(attribute)?;

        out.push_identifier(column.name.as_str())?;
        out.push_sql(op);
        match value {
            Value::String(s) => {
                let s = if starts_with {
                    format!("{}%", s)
                } else {
                    format!("%{}", s)
                };
                out.push_bind_param::<Text, _>(&s)?
            }
            Value::Bool(_)
            | Value::BigInt(_)
            | Value::Bytes(_)
            | Value::BigDecimal(_)
            | Value::Int(_)
            | Value::List(_)
            | Value::Null => {
                return Err(UnsupportedFilter {
                    filter: op.to_owned(),
                    value: value.clone(),
                }
                .into());
            }
        }
        Ok(())
    }
}

impl<'a> QueryFragment<Pg> for QueryFilter<'a> {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        use EntityFilter::*;
        match &self.filter {
            And(filters) => self.binary_op(filters, " and ", out)?,
            Or(filters) => self.binary_op(filters, " or ", out)?,

            Contains(attr, value) => self.contains(attr, value, false, out)?,
            NotContains(attr, value) => self.contains(attr, value, true, out)?,

            Equal(attr, value) => self.equals(attr, value, false, out)?,
            Not(attr, value) => self.equals(attr, value, true, out)?,

            GreaterThan(attr, value) => self.compare(attr, value, " > ", out)?,
            LessThan(attr, value) => self.compare(attr, value, " < ", out)?,
            GreaterOrEqual(attr, value) => self.compare(attr, value, " >= ", out)?,
            LessOrEqual(attr, value) => self.compare(attr, value, " <= ", out)?,

            In(attr, values) => self.in_array(attr, values, false, out)?,
            NotIn(attr, values) => self.in_array(attr, values, true, out)?,

            StartsWith(attr, value) => {
                self.starts_or_ends_with(attr, value, " like ", true, out)?
            }
            NotStartsWith(attr, value) => {
                self.starts_or_ends_with(attr, value, " not like ", true, out)?
            }
            EndsWith(attr, value) => self.starts_or_ends_with(attr, value, " like ", false, out)?,
            NotEndsWith(attr, value) => {
                self.starts_or_ends_with(attr, value, " not like ", false, out)?
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Constructor)]
pub struct FindQuery<'a> {
    mapping: &'a Mapping,
    entity: &'a str,
    id: &'a str,
}

impl<'a> FindQuery<'a> {
    fn object_query(&self, table: &'a Table, mut out: AstPass<Pg>) -> QueryResult<()> {
        // Generate
        //    select '..' as entity, to_jsonb(e.*) as data from schema.table e where id = $1
        out.push_sql("select ");
        out.push_bind_param::<Text, _>(&self.entity)?;
        out.push_sql(" as entity, to_jsonb(e.*) as data\n");
        out.push_sql("  from ");
        out.push_identifier(&self.mapping.schema)?;
        out.push_sql(".");
        out.push_identifier(table.name.as_str())?;
        out.push_sql(" e\n where ");
        out.push_identifier(crate::mapping::PRIMARY_KEY_COLUMN)?;
        out.push_sql(" = ");
        out.push_bind_param::<Text, _>(&self.id)
    }
}

impl<'a> QueryFragment<Pg> for FindQuery<'a> {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        if let Some(table) = self.mapping.table_for_entity(self.entity).ok() {
            self.object_query(table, out)
        } else if let Some(tables) = self.mapping.interfaces.get(self.entity) {
            for (i, table) in tables.iter().enumerate() {
                if i > 0 {
                    out.push_sql("\nunion all\n");
                }
                self.object_query(table, out.reborrow())?;
            }
            Ok(())
        } else {
            Err(diesel::result::Error::QueryBuilderError(Box::new(
                StoreError::UnknownTable(self.entity.to_owned()).compat(),
            )))
        }
    }
}

impl<'a> QueryId for FindQuery<'a> {
    type QueryId = ();

    const HAS_STATIC_QUERY_ID: bool = false;
}

impl<'a> LoadQuery<PgConnection, EntityData> for FindQuery<'a> {
    fn internal_load(self, conn: &PgConnection) -> QueryResult<Vec<EntityData>> {
        conn.query_by_name(&self)
    }
}

impl<'a, Conn> RunQueryDsl<Conn> for FindQuery<'a> {}

#[derive(Debug, Clone, Constructor)]
pub struct InsertQuery<'a> {
    schema_name: &'a str,
    table: &'a Table,
    key: &'a EntityKey,
    entity: Entity,
}

impl<'a> QueryFragment<Pg> for InsertQuery<'a> {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        // Construct a query
        //   insert into schema.table(column, ...)
        //   values ($1, ...)
        // and convert and bind the entity's values into it
        out.push_sql("insert into ");
        out.push_identifier(self.schema_name)?;
        out.push_sql(".");
        out.push_identifier(self.table.name.as_str())?;

        out.push_sql("(");
        for (i, column) in self
            .table
            .columns
            .iter()
            .filter(|column| !column.is_derived())
            .enumerate()
        {
            if self.entity.contains_key(&column.field) {
                if i > 0 {
                    out.push_sql(", ");
                }
                out.push_identifier(column.name.as_str())?;
            } else {
                if !column.is_nullable() {
                    return Err(diesel::result::Error::QueryBuilderError(
                        format!(
                            "can not insert entity {}[{}] since value for {} is missing",
                            self.key.entity_type, self.key.entity_id, column.field
                        )
                        .into(),
                    ));
                }
            }
        }

        out.push_sql(")\nvalues(");
        for (i, column) in self
            .table
            .columns
            .iter()
            .filter(|column| !column.is_derived())
            .enumerate()
        {
            if let Some(value) = self.entity.get(&column.field) {
                if i > 0 {
                    out.push_sql(", ");
                }
                QueryValue(value, column.column_type).walk_ast(out.reborrow())?;
            }
        }
        out.push_sql(")");
        Ok(())
    }
}

impl<'a> QueryId for InsertQuery<'a> {
    type QueryId = ();

    const HAS_STATIC_QUERY_ID: bool = false;
}

impl<'a, Conn> RunQueryDsl<Conn> for InsertQuery<'a> {}

#[derive(Debug, Clone, Constructor)]
pub struct ConflictingEntityQuery<'a> {
    mapping: &'a Mapping,
    entities: &'a Vec<&'a String>,
    entity_id: &'a String,
}

impl<'a> QueryFragment<Pg> for ConflictingEntityQuery<'a> {
    fn walk_ast(&self, mut out: AstPass<Pg>) -> QueryResult<()> {
        out.unsafe_to_cache_prepared();

        // Construct a query
        //   select 'Type1' as entity from schema.table1 where id = $1
        //   union all
        //   select 'Type2' as entity from schema.table2 where id = $1
        //   union all
        //   ...
        for (i, entity) in self.entities.iter().enumerate() {
            if i > 0 {
                out.push_sql("\nunion all\n");
            }
            out.push_sql("select ");
            out.push_bind_param::<Text, _>(entity)?;
            out.push_sql(" as entity from ");
            out.push_identifier(&self.mapping.schema)?;
            out.push_sql(".");
            let table = self
                .mapping
                .table_for_entity(entity)
                .map_err(|e| diesel::result::Error::QueryBuilderError(e.to_string().into()))?;
            out.push_identifier(table.name.as_str())?;
            out.push_sql(" where id = ");
            out.push_bind_param::<Text, _>(self.entity_id)?;
        }
        Ok(())
    }
}

impl<'a> QueryId for ConflictingEntityQuery<'a> {
    type QueryId = ();

    const HAS_STATIC_QUERY_ID: bool = false;
}

#[derive(QueryableByName)]
pub struct ConflictingEntityData {
    #[sql_type = "Text"]
    pub entity: String,
}

impl<'a> LoadQuery<PgConnection, ConflictingEntityData> for ConflictingEntityQuery<'a> {
    fn internal_load(self, conn: &PgConnection) -> QueryResult<Vec<ConflictingEntityData>> {
        conn.query_by_name(&self)
    }
}

impl<'a, Conn> RunQueryDsl<Conn> for ConflictingEntityQuery<'a> {}
