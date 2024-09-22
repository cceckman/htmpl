//! Visitor for an HTML tree.

use std::collections::HashMap;

use ego_tree::{NodeId, NodeRef};
use scraper::{html, Node};

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

impl VariableTable {
    /// Look up the variable.
    fn get<'a>(&'a self, name: impl AsRef<str>) -> Option<&'a QueryResult> {
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

fn do_query(_scopes: &[Scope]) -> Result<QueryResult, crate::Error> {
    Ok(vec![[("query".to_owned(), "unimplemented".to_owned())]
        .into_iter()
        .collect::<HashMap<String, _>>()])
}

/// Parse the HTML tree, replacing htmpl elements and attributes.
pub fn evaluate_template(s: impl AsRef<str>) -> Result<String, crate::Error> {
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
                let n: NodeRef<'_, Node> =
                    h.tree.get(node).expect("retrieved with invalid node ID");
                if let Node::Element(element) = n.value() {
                    match element.name.local.as_ref() {
                        "htmpl-insert" => {
                            let query = element
                                .attr("query")
                                .ok_or(Error::MissingAttr("htmpl-insert", "query"))?;
                            let _result = vars
                                .get(query)
                                .ok_or(Error::MissingQuery("htmpl-insert", query.to_owned()))?;
                            // TODO: Extract columns, actually replace
                        }
                        "htmpl-query" => {
                            // TODO: Can we add line number information?
                            let name = element
                                .attr("name")
                                .ok_or(crate::Error::MissingAttr("htmpl-query", "name"))?;
                            let result = do_query(&scopes)?;
                            if let Some(s) = scopes.last_mut() {
                                s.push(name.to_owned())
                            }
                            vars.bind_shadow(name, result);

                            // We've performed the side effects of htmpl-query.
                            // Delete the node from the DOM.
                            let _ = h.tree.get_mut(node).map(|mut v| v.detach());

                            // We don't need to do anything with the children of this element,
                            // so:
                            continue;
                        }
                        _ => {}
                    }
                }
                // Other elements, or other node types: recurse.
                // Create a new scope:
                scopes.push(Default::default());
                // Visit children, then exit the new scope;
                // push those onto the work stack in reverse order.
                work_stack.push(Step::ExitScope(node));
                let mut child = n.last_child();
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
        let _db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-insert query="q" column="uuid" />
        "#;
        let result =
            evaluate_template(TEMPLATE).expect_err("succeeded at evaluating invalid template");
        assert_eq!(result, Error::MissingQuery("htmpl-insert", "q".to_owned()));
    }

    #[test]
    fn insert_single_value() {
        let _db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT uuid FROM users WHERE name = "cceckman" LIMIT 1;
</htmpl-query>
<htmpl-insert query="q" column="uuid" />
        "#;
        let result: String = evaluate_template(TEMPLATE).expect("failed to evaluate template");
        assert_eq!(result.trim(), CCECKMAN_UUID);
    }
}
