//! Visitor for an HTML tree.

use std::rc::Rc;

use crate::queries::{Attribute, DbTable, Scope};
use ego_tree::{NodeMut, NodeRef};
use html5ever::{
    local_name, ns,
    serialize::{SerializeOpts, TraversalScope},
    QualName,
};
use rusqlite::types::Value;
use scraper::{selectable::Selectable, ElementRef, Node, Selector};

use crate::Error;

/// Recursive "visit" function.
///
/// Evaluates the source node in the provided scope,
/// adding elements under output_parent as needed.
fn visit_recurse(
    scope: &mut Scope,
    source: NodeRef<Node>,
    output_parent: &mut NodeMut<Node>,
) -> Result<(), Error> {
    if let Some(eref) = ElementRef::wrap(source) {
        visit_element(scope, eref, output_parent)
    } else {
        let mut new = output_parent.append(source.value().clone());
        let mut scope = scope.push();
        for child in source.children() {
            visit_recurse(&mut scope, child, &mut new)?;
        }
        Ok(())
    }
}

/// Visit an element node in the tree.
/// Delegates to specialized functions for htmpl-* elements.
fn visit_element(
    scope: &mut Scope,
    source: ElementRef,
    output_parent: &mut NodeMut<Node>,
) -> Result<(), Error> {
    match source.value().name.local.as_ref() {
        "htmpl-foreach" => visit_foreach(scope, source, output_parent),
        "htmpl-insert" => {
            let content = visit_insert(scope, source)?;
            output_parent.append(Node::Text(scraper::node::Text {
                text: content.into(),
            }));
            Ok(())
        }
        "htmpl-query" => scope.do_query(source),
        "htmpl-attr" => visit_attr(scope, source),
        _ => {
            let mut new = source.value().clone();
            // TODO: Consider constructing the qualified Attribute in the -attr element, and
            // cloning it here; that should do less string-cloning up-front
            for new_attr in scope.get_attrs(source.id()) {
                new.attrs.insert(
                    QualName::new(None, "".into(), new_attr.name.clone().into()),
                    new_attr.value.clone().into(),
                );
            }
            // TODO: Actually add the new attributes to the element?

            let mut new = output_parent.append(Node::Element(new));
            // Patch attributes.
            // Insert self, then recurse in a new scope.
            let mut scope = scope.push();
            for child in source.children() {
                visit_recurse(&mut scope, child, &mut new)?;
            }
            Ok(())
        }
    }
}

/// Evaluate an htmpl-insert element.
/// Returns the text with which to replace the node in the output tree.
fn visit_insert(scope: &Scope, element: ElementRef) -> Result<String, Error> {
    let query = element
        .value()
        .attr("query")
        .ok_or(Error::MissingAttr("htmpl-insert", "query"))?;
    let value = scope
        .get_single(query)
        .map_err(|e| e.set_element("htmpl-insert"))?;
    Ok(format_value(value))
}

/// Visit an htmpl-foreach node.
/// Recurses into the output tree, inserting into the output for each row.
fn visit_foreach(
    scope: &mut Scope,
    element: ElementRef,
    output_parent: &mut NodeMut<Node>,
) -> Result<(), Error> {
    let query = element
        .value()
        .attr("query")
        .ok_or(Error::MissingAttr("htmpl-foreach", "query"))?;
    let it = scope
        .for_each_row(query)
        .ok_or(Error::MissingQuery("htmpl-foreach", query.to_owned()))?;
    for mut scope in it {
        // rows * children:
        for child in element.children() {
            visit_recurse(&mut scope, child, output_parent)?;
        }
    }
    Ok(())
}

/// Evaluate an htmpl-attr element.
fn visit_attr(scope: &mut Scope, element: ElementRef) -> Result<(), Error> {
    let query = element
        .value()
        .attr("query")
        .ok_or(Error::MissingAttr("htmpl-attr", "query"))?;
    let select = element
        .value()
        .attr("select")
        .ok_or(Error::MissingAttr("htmpl-attr", "select"))?;
    let selector: Selector = Selector::parse(select)
        .map_err(|_| Error::InvalidParameter("htmpl-attr", "select".to_owned()))?;
    let attr = element
        .value()
        .attr("attr")
        .ok_or(Error::MissingAttr("htmpl-attr", "attr"))?;
    let value = scope
        .get_single(query)
        .map_err(|e| e.set_element("htmpl-attr"))?;
    let attr = Rc::new(Attribute {
        name: attr.to_owned(),
        value: format_value(value),
    });

    if let Some(parent) = element.parent().and_then(ElementRef::wrap) {
        for selected in parent.select(&selector) {
            scope.add_attr(selected.id(), attr.clone())
        }
    }

    Ok(())
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
    let h = html5ever::driver::parse_fragment(
        scraper::Html::new_fragment(),
        Default::default(),
        QualName::new(None, ns!(), local_name!("")),
        Vec::new(),
    )
    .one(s.as_ref());
    // let mut h = Html::parse_fragment(s.as_ref());

    let mut scope = Scope::new(dbs);
    let mut output = scraper::Html::new_fragment();
    visit_recurse(&mut scope, h.tree.root(), &mut output.tree.root_mut())?;

    // Scraper appears to synthesize an <html> wrapping element.
    // TODO: Make "this is a fragment" vs. "this is a whole-document" explicit,
    // so we do/don't strip the <html> element depending.
    // (Why does scraper add a root element?)
    // For now, we remove it here:
    if let Some(root) = output.select(&Selector::parse("html").unwrap()).next() {
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
        tests::{html_equal, make_test_db, CCECKMAN_UUID, OTHER_UUID},
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
    fn multi_column_requires_column_selection() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT * FROM users WHERE name = "cceckman";
</htmpl-query>
<htmpl-insert query="q" />
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect_err("unexpected success");
        if let Error::NoDefaultColumn("htmpl-insert", _, _) = result {
        } else {
            panic!("unexpected error: {}", result);
        }
    }

    #[test]
    fn error_on_invalid_column() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT * FROM users WHERE name = "cceckman";
</htmpl-query>
<htmpl-insert query="q(does-not-exist)" />
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect_err("unexpected success");
        if let Error::MissingColumn("htmpl-insert", _, _, _) = result {
        } else {
            panic!("unexpected error: {}", result);
        }
    }

    #[test]
    fn insert_named_column() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT * FROM users WHERE name = "cceckman";
</htmpl-query>
<htmpl-insert query="q(uuid)" />
        "#;
        let result: String = evaluate_template(TEMPLATE, &db).expect("failed to evaluate template");
        assert_eq!(result.trim(), CCECKMAN_UUID);
    }

    #[test]
    fn insert_default_column() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT uuid FROM users WHERE name = "cceckman";
</htmpl-query>
<htmpl-insert query="q" />
        "#;
        let result: String = evaluate_template(TEMPLATE, &db).expect("failed to evaluate template");
        assert_eq!(result.trim(), CCECKMAN_UUID);
    }

    #[test]
    fn insert_requires_single_row() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT uuid FROM users;
</htmpl-query>
<htmpl-insert query="q" />
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect_err("unexpected success");
        if let Error::Cardinality("htmpl-insert", _, _, _) = result {
        } else {
            panic!("unexpected error: {}", result);
        }
    }

    #[test]
    fn shadow_inner_scope() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<div>
    <htmpl-query name="q">
    SELECT uuid FROM users WHERE name = "ddedkman";
    </htmpl-query>
    <div>
        <htmpl-query name="q">
        SELECT uuid FROM users WHERE name = "cceckman";
        </htmpl-query>
        <htmpl-insert query="q" />
    </div>
    <htmpl-insert query="q" />
</div>
        "#;
        let result: String = evaluate_template(TEMPLATE, &db).expect("failed to evaluate template");
        let trimmed: String = result
            .chars()
            .filter(|v| !char::is_whitespace(*v))
            .collect();
        assert_eq!(
            trimmed,
            format!("<div><div>{}</div>{}</div>", CCECKMAN_UUID, OTHER_UUID)
        );
    }

    #[test]
    fn foreach_multiple() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT * FROM users;
</htmpl-query>
<htmpl-foreach query="q">
<htmpl-insert query="q(uuid)" /> <htmpl-insert query="q(name)" />
</htmpl-foreach>
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect("unexpected error");
        assert!(
            result.contains(&format!("{} cceckman", CCECKMAN_UUID)),
            "output does not contain cceckman:\n---\n{}\n---\n",
            result
        );
        assert!(
            result.contains(&format!("{} ddedkman", OTHER_UUID)),
            "output does not contain other:\n---\n{}\n---\n",
            result
        );
    }

    #[test]
    fn foreach_empty() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT * FROM users WHERE name = "noone";
</htmpl-query>
<htmpl-foreach query="q">
<htmpl-insert query="q(uuid)" /> <htmpl-insert query="q(name)" />
</htmpl-foreach>
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect("unexpected error");
        assert!(!result.contains(&format!("{} cceckman", CCECKMAN_UUID)));
        assert!(!result.contains(&format!("{} ddedkman", OTHER_UUID)));
        assert_eq!(result.trim(), "");
    }

    #[test]
    fn single_query_parameter() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="get_uuid" >
SELECT uuid FROM users;
</htmpl-query>
<htmpl-foreach query="get_uuid">
<htmpl-query name="get_name" :uuid="get_uuid(uuid)" >
SELECT name FROM users WHERE uuid = :uuid
</htmpl-query>
<htmpl-insert query="get_name" />
</htmpl-foreach>
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect("unexpected error");
        assert!(result.contains("cceckman"));
        assert!(result.contains("ddedkman"));
    }

    #[test]
    fn constant() {
        let db = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="admin_name">
SELECT "cceckman" AS admin_name;
</htmpl-query>
<htmpl-query name="admin_uuid" :name="admin_name">
SELECT uuid FROM users WHERE name = :name;
</htmpl-query>
<htmpl-insert query="admin_uuid" />
        "#;
        let result = evaluate_template(TEMPLATE, &db).expect("unexpected error");
        assert_eq!(result.trim(), format!("{}", CCECKMAN_UUID));
    }

    #[test]
    fn single_attr() {
        let conn = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">SELECT name, (uuid || " name") AS uuid_class FROM users ORDER BY name ASC LIMIT 1;</htmpl-query>
<htmpl-attr select=".name" query="q(uuid_class)" attr="class" />
<div class="name"><htmpl-insert query="q(name)" /></div>
"#;
        let result = evaluate_template(TEMPLATE, &conn).unwrap();
        assert_eq!(
            result.trim(),
            r#"<div class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name">cceckman</div>"#
        );
    }

    #[test]
    fn attr_selector() {
        let conn = make_test_db();
        const TEMPLATE: &str = r#"
<htmpl-query name="q">SELECT name, (uuid || " name") AS uuid_class FROM users ORDER BY name ASC LIMIT 1;</htmpl-query>
<htmpl-attr select=".name" query="q(uuid_class)" attr="class" />
<div class="name"><a href="https://cceckman.com"><htmpl-insert query="q(name)" /></a></div>
<div><a class="name" href="https://cceckman.com"><htmpl-insert query="q(name)" /></a></div>"#;
        let result = evaluate_template(TEMPLATE, &conn).unwrap();
        // Don't depend on attribute order:
        html_equal(
            result.trim(),
            r#"
<div class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name"><a href="https://cceckman.com">cceckman</a></div>
<div><a href="https://cceckman.com" class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name">cceckman</a></div> "#.trim(),
        );
    }
}
