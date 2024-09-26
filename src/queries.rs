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

use std::{collections::HashMap, rc::Rc};

use rusqlite::{params, types::Value};
use scraper::ElementRef;

use crate::Error;

/// Result of performing a database query:
/// Rows, then column name -> values.
type QueryResult = Vec<HashMap<String, Value>>;

/// Databases available for querying.
pub type DbTable = rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct Scope<'a> {
    dbs: &'a DbTable,
    bindings: HashMap<String, Rc<QueryResult>>,
}

impl<'a> Scope<'a> {
    /// Create a new scope where queries operate on the provided databases.
    pub fn new(dbs: &'a DbTable) -> Scope<'a> {
        Scope {
            dbs,
            bindings: Default::default(),
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
}

impl Scope<'_> {
    /// Look up the results of the named query.
    pub fn get(&self, name: impl AsRef<str>) -> Option<&QueryResult> {
        self.bindings.get(name.as_ref()).map(|v| &**v)
    }

    /// Perform the query described in `element`.
    /// Binds the query results to the query given in the `name` attribute.
    pub fn do_query(&mut self, element: ElementRef) -> Result<(), Error> {
        let name = element
            .attr("name")
            .ok_or(Error::MissingAttr("htmpl-query", "name"))?;
        let note_err = |e| Error::Sql(name.to_owned(), e);
        let content = element.text().collect::<Vec<_>>().join(" ");

        let mut st = self
            .dbs
            .prepare(&content)
            .map_err(|e| Error::Sql(name.to_owned(), e))?;
        let names: Vec<String> = (0..st.column_count())
            .filter_map(|i| st.column_name(i).map(str::to_owned).ok())
            .collect();
        // TODO: Pass params from vars
        let result: rusqlite::Result<QueryResult> = st
            .query(params![])
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
