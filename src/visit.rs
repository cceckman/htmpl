//! Visitor for an HTML tree.

use std::collections::HashMap;

use ego_tree::{NodeId, NodeMut, NodeRef};
use html5ever::{
    local_name, ns,
    serialize::{SerializeOpts, TraversalScope},
    QualName,
};
use rusqlite::{params, types::Value};
use scraper::{ElementRef, Node, Selector};

use crate::Error;

// Step in traversing the tree.
enum Step {
    /// Visit the provided element.
    Visit(NodeId),
    /// Exit the scope of the given element, dropping variables
    ExitScope(NodeId),
}

/// Result of performing a database query:
/// Rows, then column name -> values.
type QueryResult = Vec<HashMap<String, Value>>;

/// Variable binding.
#[derive(Debug)]
struct Binding {
    result: QueryResult,
    shadowed: Option<Box<Binding>>,
}

/// Variables present in a given scope.
type Scope = Vec<String>;

/// The variables in scope at a given point.
#[derive(Default, Debug)]
struct VariableTable(HashMap<String, Binding>);

/// Databases available for querying.
type DbTable = rusqlite::Connection;

impl VariableTable {
    /// Look up the variable.
    fn get(&self, name: impl AsRef<str>) -> Option<&QueryResult> {
        self.0.get(name.as_ref()).map(|v| &v.result)
    }

    /// Add a variable binding, shadowing if it's already bound.
    fn bind_shadow(&mut self, name: &str, result: QueryResult) {
        let shadowed = self.0.remove(name).map(Box::new);
        self.0.insert(name.to_owned(), Binding { result, shadowed });
    }

    /// Remove the current bindings of these variables, un-shadowing if shadowed.
    fn pop_scope(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            if let Some(binding) = self.0.remove(&name).and_then(|v| v.shadowed.map(|v| *v)) {
                self.0.insert(name, binding);
            }
        }
    }
}

/// Allow reborrowing a NodeMut as a NodeRef.
fn node_ref<'a, T>(nm: &'a mut NodeMut<'_, T>) -> NodeRef<'a, T> {
    let id = nm.id();
    unsafe { nm.tree().get_unchecked(id) }
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

fn do_query(
    element: ElementRef,
    vars: &mut VariableTable,
    _current_scope: &mut Scope,
    dbs: &DbTable,
) -> Result<(), Error> {
    let name = element
        .attr("name")
        .ok_or(Error::MissingAttr("htmpl-query", "name"))?;
    let note_err = |e| Error::Sql(name.to_owned(), e);
    let content = element.text().collect::<Vec<_>>().join(" ");

    let mut st = dbs
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
    vars.bind_shadow(name, result);
    Ok(())
}

/// Visit a node in the tree.
/// Returns true if recursion is required.
/// TODO: Extract to a struct.
fn visit(
    mut nm: NodeMut<'_, Node>,
    vars: &mut VariableTable,
    current_scope: &mut Scope,
    dbs: &DbTable,
) -> Result<bool, Error> {
    if let Node::Element(element) = nm.value() {
        match element.name.local.as_ref() {
            "htmpl-insert" => {
                let new_content = visit_insert(element, vars)?;
                // TODO: Just replacing with text for now.
                *nm.value() = Node::Text(scraper::node::Text {
                    text: new_content.parse().unwrap(),
                });

                Ok(false)
            }
            "htmpl-query" => {
                let elem = ElementRef::wrap(node_ref(&mut nm))
                    .expect("internal failure: already asserted node is-element");

                // TODO: Can we add line number information?
                do_query(elem, vars, current_scope, dbs)?;

                // We've performed the side effects of htmpl-query.
                // Delete the node from the DOM.
                nm.detach();

                // We don't need to do anything with the children of this element, so:
                Ok(false)
            }
            _ => Ok(true),
        }
    } else {
        // All other node types, recurse to children.
        Ok(true)
    }
}

/// Visit a node in the tree.
/// Returns true if recursion is required.
fn visit_insert(element: &scraper::node::Element, vars: &VariableTable) -> Result<String, Error> {
    let query = element
        .attr("query")
        .ok_or(Error::MissingAttr("htmpl-insert", "query"))?;
    let result = vars
        .get(query)
        .ok_or(Error::MissingQuery("htmpl-insert", query.to_owned()))?;
    // An insert cannot flatten results; the length has to be 1.
    if result.len() != 1 {
        return Err(Error::Cardinality(
            "htmpl-insert",
            query.to_owned(),
            result.len(),
            1,
        ));
    }
    let result = &result[0];
    let fmt_columns = || {
        format!(
            "\"{}\"",
            result
                .iter()
                .map(|(k, _v)| k.to_owned())
                .collect::<Vec<_>>()
                .join(",")
        )
    };

    // Extract the relevant column: explicit, or implicit single column.
    let value = if let Some(v) = element.attr("column") {
        // An explicit column was specified.
        result.get(v).ok_or_else(|| {
            Error::MissingColumn(
                "htmpl-insert",
                query.to_owned(),
                fmt_columns(),
                v.to_owned(),
            )
        })?
    } else {
        (if result.len() == 1 {
            result.iter().next().map(|(_k, v)| v)
        } else {
            None
        })
        .ok_or_else(|| Error::NoDefaultColumn("htmpl-insert", query.to_owned(), fmt_columns()))?
    };
    Ok(format_value(value))
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Integer(i) => format!("{}", i),
        Value::Real(f) => format!("{}", f),
        Value::Text(t) => t.clone(),
        Value::Blob(b) => format!(
            "[{}]",
            b.iter()
                .map(|b| format!("{:2x}", b))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Parse the HTML tree, replacing htmpl elements and attributes.
pub fn evaluate_template(s: impl AsRef<str>, dbs: &DbTable) -> Result<String, Error> {
    // scraper::parse_fragment impugns an <html> element into the root, which isn't necessarily
    // true for us.
    // Try to parse without adding an <html>.
    // ...doesn't work.
    use html5ever::namespace_url;
    use html5ever::tendril::TendrilSink;
    let mut h = html5ever::driver::parse_fragment(
        scraper::Html::new_fragment(),
        Default::default(),
        QualName::new(None, ns!(), local_name!("")),
        Vec::new(),
    )
    .one(s.as_ref());
    // let mut h = Html::parse_fragment(s.as_ref());

    let mut vars = VariableTable::default();
    let mut scopes: Vec<Scope> = vec![Default::default()];
    let mut work_stack: Vec<Step> = Vec::default();

    // Start by pushing the top-level element(s) onto the work stack.
    work_stack.push(Step::Visit(h.tree.root().id()));
    // We modify the tree in-place as we go.
    // NodeIds are stable: a node can be detached/orphaned, but never removed.
    // Now work:
    // TODO: Replace this with a root.traverse() invocation? Is that the same?
    while let Some(step) = work_stack.pop() {
        match step {
            Step::ExitScope(_e) => {
                // Get rid of the variables from this scope:
                if let Some(v) = scopes.pop() {
                    vars.pop_scope(v);
                }
            }
            Step::Visit(node) => {
                {
                    let nm: NodeMut<'_, Node> = h
                        .tree
                        .get_mut(node)
                        .expect("retrieved with invalid node ID");
                    visit(
                        nm,
                        &mut vars,
                        scopes.last_mut().expect("scopes must always be nonempty"),
                        dbs,
                    )?;
                }

                // Other elements, or other node types: recurse.
                // Create a new scope:
                scopes.push(Default::default());
                // Visit children, then exit the new scope;
                // push those onto the work stack in reverse order.
                work_stack.push(Step::ExitScope(node));
                let mut child = h.tree.get(node).and_then(|v| v.last_child());
                while let Some(c) = child {
                    work_stack.push(Step::Visit(c.id()));
                    child = c.prev_sibling();
                }
            }
        }
    }

    // The parsing routine synthesizes an <html> wrapping element.
    // TODO: Make "this is a fragment" vs. "this is a whole-document" explicit,
    // so we do/don't strip the <html> element depending.
    // (Why does scraper add a root element?)
    // For now, we remove it here:
    if let Some(root) = h.select(&Selector::parse("html").unwrap()).next() {
        // Lifted from scraper::Html::serialize(), but with different options.
        let mut buf = Vec::new();
        // TODO: Derferred (I/O) serialization?
        html5ever::serialize(
            &mut buf,
            &root,
            SerializeOpts {
                scripting_enabled: false, // What does this do?
                traversal_scope: TraversalScope::ChildrenOnly(None),
                // traversal_scope: TraversalScope::IncludeNode,
                create_missing_parent: false,
            },
        )
        .map_err(Error::Serialize)?;
        return Ok(String::from_utf8(buf).unwrap());
    }
    panic!("unexpected end of function: no root element");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tests::{make_test_db, CCECKMAN_UUID},
        Error,
    };

    #[test]
    fn missing_query() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-insert query="q" />
        "#;
        let result =
            evaluate_template(TEMPLATE, &db).expect_err("succeeded at evaluating invalid template");
        assert_eq!(result, Error::MissingQuery("htmpl-insert", "q".to_owned()));
    }

    #[test]
    fn insert_single_value() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT uuid FROM users WHERE name = "cceckman" LIMIT 1;
</htmpl-query>
<htmpl-insert query="q" />
        "#;
        let result: String = evaluate_template(TEMPLATE, &db).expect("failed to evaluate template");
        assert_eq!(result.trim(), CCECKMAN_UUID);
    }
}
