//! Query handling for htmpl.
//!
//! The `htmpl-query` element describes a SQL query
//! on a read-only database. The query itself is
//! between the start and end tags; the attributes
//! describe the name to give the query.
//!
//! When htmpl evaluates the `htmpl-query` element,
//! it executes the query, and does not replace/replicate
//! it in the output HTML.
//! htmpl binds the results of the query to the name
//! given by the `query` attribute.
//!
//! Queries are scoped according to the HTML hierarchy:
//!
//! ```html
//! <div>
//!     <htmpl-query query="foo">...</htmpl-query>
//!     <!-- Can use "foo" here -->
//! </div>
//! <!-- Cannot use "foo" here -->
//! ```
//!
//! Queries shadow according to scope:
//!
//! ```html
//! <htmpl-query query="foo">SELECT name FROM people;</htmpl-query>
//! <div>
//!     <!-- "foo" has column "name" -->
//!     <htmpl-query query="foo">SELECT id FROM people;</htmpl-query>
//!     <!-- "foo" has column "id" -->
//! </div>
//! <!-- "foo" has column "id" -->
//! ```
//!

use std::{collections::HashMap, ops::Deref, rc::Rc};

use ego_tree::NodeId;
use rusqlite::{types::Value, ToSql};
use scraper::ElementRef;

use crate::Error;

/// Result of performing a database query:
/// Rows, then column name -> values.
type QueryResult = Vec<HashMap<String, Value>>;

/// Databases available for querying.
pub type DbTable = rusqlite::Connection;

/// An attribute added with the htmpl-attr element.
#[derive(Debug, PartialEq, Eq)]
pub struct Attribute {
    /// TODO: Should this be an Atom or similar? Something from scraper?
    pub name: String,
    pub value: String,
}

/// Data local to the current scope.
#[derive(Debug, Clone)]
pub struct Scope<'a> {
    dbs: &'a DbTable,
    bindings: HashMap<String, Rc<QueryResult>>,
    attrs: HashMap<NodeId, Vec<Rc<Attribute>>>,
}

impl<'a> Scope<'a> {
    /// Create a new scope where queries operate on the provided databases.
    pub fn new(dbs: &'a DbTable) -> Scope<'a> {
        Scope {
            dbs,
            bindings: Default::default(),
            attrs: Default::default(),
        }
    }

    /// Create a new scope from the current one.
    pub fn push(&self) -> Scope {
        self.clone()
    }

    /// Generate a new scope for each row in the named query.
    /// In each sub-scope, the named query is filtered down to a single row.
    pub fn for_each_row(&self, query_name: impl AsRef<str>) -> Option<RowIterator<'a>> {
        let query_name = query_name.as_ref();
        let query = self.bindings.get(query_name)?.clone();
        Some(RowIterator {
            query_name: query_name.to_owned(),
            query,
            i: 0,
            parent_scope: self.clone(),
        })
    }

    /// Add an attribute binding.
    pub fn add_attr(&mut self, node: NodeId, attr: Rc<Attribute>) {
        self.attrs.entry(node).or_default().push(attr)
    }

    /// Get all attributes for a given node.
    pub fn get_attrs(&self, node: NodeId) -> &[impl Deref<Target = Attribute>] {
        self.attrs.get(&node).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Parse a parameter name into a query_name and optional column.
fn parse_specifier(s: &str) -> Result<(&str, Option<&str>), Error> {
    let mk_err = || Error::InvalidParameter("", s.to_owned());
    let (query_name, tail) = match s.split_once("(") {
        None => return Ok((s, None)),
        Some(tail) => tail,
    };
    let (column_name, zero) = tail.split_once(")").ok_or_else(mk_err)?;
    if query_name.is_empty() || column_name.is_empty() || !zero.is_empty() {
        return Err(mk_err());
    }
    Ok((query_name, Some(column_name)))
}

impl Scope<'_> {
    /// Look up the results of the named query.
    pub fn get(&self, name: impl AsRef<str>) -> Result<&QueryResult, Error> {
        self.bindings
            .get(name.as_ref())
            .map(|v| &**v)
            .ok_or_else(|| Error::MissingQuery("", name.as_ref().to_owned()))
    }

    /// Gets a single value from a specifier.
    /// The specifier may be of the form:
    /// - query_name, if the query's results are a single row and single column
    /// - query_name(column_name), if the query's results are a single row
    pub fn get_single(&self, specifier: impl AsRef<str>) -> Result<&Value, Error> {
        let (query_name, column_name) = parse_specifier(specifier.as_ref())?;
        let q = self.get(query_name)?;
        let row = match q.len() {
            1 => &q[0],
            _ => return Err(Error::Cardinality("", query_name.to_owned(), q.len(), 1)),
        };
        let fmt_columns = || {
            format!(
                "\"{}\"",
                row.iter()
                    .map(|(k, _v)| k.to_owned())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        // Extract the relevant column: explicit, or implicit single column.
        let value = if let Some(v) = column_name {
            // An explicit column was specified; try it out.
            row.get(v).ok_or_else(|| {
                Error::MissingColumn("", query_name.to_owned(), fmt_columns(), v.to_owned())
            })?
        } else {
            (if row.len() == 1 {
                row.iter().next().map(|(_k, v)| v)
            } else {
                None
            })
            .ok_or_else(|| Error::NoDefaultColumn("", query_name.to_owned(), fmt_columns()))?
        };
        Ok(value)
    }

    /// Perform the query described in `element`.
    /// Binds the query results to the query given in the `name` attribute.
    ///
    /// TODO: Document parameter usage --
    /// - Use the ":param_name" format for parameter names
    /// - Use attributes named ":parameter_name", which name the variable to use
    /// Attributes starting with a colon are valid in XML, i.e. for custom components:
    /// https://www.w3.org/TR/xml/#NT-Name
    /// https://stackoverflow.com/questions/925994/what-characters-are-allowed-in-an-html-attribute-name
    pub fn do_query(&mut self, element: ElementRef) -> Result<(), Error> {
        let name = element
            .attr("name")
            .ok_or(Error::MissingAttr("htmpl-query", "name"))?;
        let note_err = |e| Error::Sql(name.to_owned(), e);
        let content = element
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned();
        let mut st = self
            .dbs
            .prepare(&content)
            .map_err(|e| Error::Sql(name.to_owned(), e))?;
        let names: Vec<String> = (0..st.column_count())
            .filter_map(|i| st.column_name(i).map(str::to_owned).ok())
            .collect();
        // Column names are (apparently) zero-indexed;
        // parameter names are one-indexed.
        let param_names: Vec<String> = (0..st.parameter_count())
            .filter_map(|i| st.parameter_name(i + 1).map(str::to_owned))
            .collect();
        let params: Result<Vec<(&str, &dyn ToSql)>, Error> = param_names
            .iter()
            .map(|name| {
                let query = element
                    .attr(&name)
                    .ok_or_else(|| Error::MissingParameter("", name.clone()))?;
                let value: &dyn ToSql = self.get_single(query)?;
                Ok((name.as_str(), value))
            })
            .collect();
        let params = params.map_err(|e| e.set_element("htmpl-query"))?;

        // TODO: For some reson, making this Result<QueryResult> is discarding one of the entries of the Vec.
        // Something about aggregating Vec<HashMap> maybe?
        let result: rusqlite::Result<QueryResult> = st
            .query(params.as_slice())
            .map_err(note_err)?
            .mapped(|row| row_to_hash(&names, row))
            .collect();
        let result = result.map_err(note_err)?;
        self.bindings.insert(name.to_owned(), Rc::new(result));
        Ok(())
    }
}

/// An iterator over the rows of a query.
/// In each returned scope, the query named in 'query' is bound to a different row of the result.
pub struct RowIterator<'a> {
    query_name: String,
    query: Rc<QueryResult>,
    i: usize,
    parent_scope: Scope<'a>,
}

impl<'a> Iterator for RowIterator<'a> {
    type Item = Scope<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let row = self.query.get(self.i)?;
        self.i += 1;
        let mut new = self.parent_scope.clone();
        new.bindings
            .insert(self.query_name.clone(), Rc::new(vec![row.clone()]));
        Some(new)
    }
}

/// Decode a single row into a column->value hashmap.
fn row_to_hash(
    columns: &[impl AsRef<str>],
    row: &rusqlite::Row,
) -> rusqlite::Result<HashMap<String, Value>> {
    columns
        .iter()
        .enumerate()
        .map(|(i, name)| -> rusqlite::Result<(String, Value)> {
            let v = row.get(i)?;
            Ok((name.as_ref().to_owned(), v))
        })
        .collect()
}
