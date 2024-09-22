//! Visitor for an HTML tree.

use std::collections::HashMap;

use ego_tree::{NodeId, NodeMut, NodeRef};
use scraper::{html, Element, ElementRef, Node};

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
type QueryResult = Vec<HashMap<String, String>>;

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

fn do_query(
    element: ElementRef,
    vars: &mut VariableTable,
    current_scope: &mut Scope,
    _dbs: &DbTable,
) -> Result<(), crate::Error> {
    let name = element
        .attr("name")
        .ok_or(crate::Error::MissingAttr("htmpl-query", "name"))?;

    let result = vec![[("query".to_owned(), "unimplemented".to_owned())]
        .into_iter()
        .collect::<HashMap<String, _>>()];

    current_scope.push(name.to_owned());
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
                let _ = do_query(elem, vars, current_scope, dbs)?;

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
fn visit_insert<'a>(
    element: &scraper::node::Element,
    vars: &'a VariableTable,
) -> Result<&'a str, Error> {
    let query = element
        .attr("query")
        .ok_or(Error::MissingAttr("htmpl-insert", "query"))?;
    let result = vars
        .get(query)
        .ok_or(Error::MissingQuery("htmpl-insert", query.to_owned()))?;
    // Cannot flatten results.
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

    // Extract the relevant column
    let value = if let Some(v) = element.attr("column") {
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
    Ok(value)
}

/// Parse the HTML tree, replacing htmpl elements and attributes.
pub fn evaluate_template(s: impl AsRef<str>, dbs: &DbTable) -> Result<String, crate::Error> {
    // Tree to traverse:
    let mut h = html::Html::parse_fragment(s.as_ref());
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
    Ok(h.html())
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
