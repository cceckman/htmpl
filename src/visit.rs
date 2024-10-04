//! Visitor for an HTML tree.

use std::rc::Rc;

use crate::queries::{Attribute, DbTable, Scope};
use ego_tree::{NodeMut, NodeRef};
use html5ever::{
    local_name, ns,
    serialize::{SerializeOpts, TraversalScope},
    tokenizer::TokenizerOpts,
    tree_builder::TreeBuilderOpts,
    QualName,
};
use rusqlite::types::{Value, ValueRef};
use scraper::{selectable::Selectable, ElementRef, Node, Selector};

use crate::Error;

/// Returns true if the database value is truthy.
fn truthy(v: ValueRef) -> bool {
    match v {
        ValueRef::Null => false,
        ValueRef::Integer(i) => i != 0,
        ValueRef::Real(f) => !(f.is_nan() || f == 0.0 || f == -0.0),
        ValueRef::Text(s) => !s.is_empty(),
        ValueRef::Blob(b) => !b.is_empty(),
    }
}

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
    tracing::debug!("element: {}", source.value().name.local.as_ref());
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
        "htmpl-if" => visit_if(scope, source, output_parent),
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
        .ok_or(Error::MissingQuery("htmpl-foreach", query.to_owned()))?
        .enumerate();
    for (i, mut scope) in it {
        let _iteration = tracing::debug_span!("foreach", "i={}", i).entered();
        // rows * children:
        for child in element.children() {
            visit_recurse(&mut scope, child, output_parent)?;
        }
    }
    Ok(())
}

/// Visit an htmpl-if node.
/// If the expression is true, recurse into the subtree.
fn visit_if(
    scope: &mut Scope,
    element: ElementRef,
    output_parent: &mut NodeMut<Node>,
) -> Result<(), Error> {
    let t = element.value().attr("true");
    let f = element.value().attr("false");
    if t.is_some() && f.is_some() {
        return Err(Error::MultipleConditions(format!("{:?}", element)));
    }

    let specifier = t
        .or(f)
        .ok_or(Error::MissingAttr("htmpl-if", "true= or false="))?;

    let it = scope
        .get_single(specifier)
        .map_err(|e| e.set_element("htmpl-if"))?;
    let truthiness = truthy(it.into());
    if t.is_some() && truthiness || f.is_some() && !truthiness {
        let mut scope = scope.push();
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
            tracing::debug!("add_attr {:?}", selected);
            scope.add_attr(selected.id(), attr.clone())
        }
    } else {
        tracing::error!("htmpl-attr with no parent: {:?}", element);
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
        html5ever::ParseOpts {
            tokenizer: TokenizerOpts {
                exact_errors: true,
                ..TokenizerOpts::default()
            },
            tree_builder: TreeBuilderOpts {
                exact_errors: true,
                // Enable "scripting" since we have custom elements
                scripting_enabled: true,
                ..TreeBuilderOpts::default()
            },
        },
        QualName::new(None, ns!(html), local_name!("body")),
        Vec::new(),
    )
    .one(s.as_ref());
    if !h.errors.is_empty() {
        return Err(Error::HtmlParse(h.errors.join("; ")));
    }
    tracing::debug!("parse errors: {:?}", h.errors);
    tracing::debug!("quirks: {:?}", h.quirks_mode);
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
    use rusqlite::types::ValueRef;

    use crate::visit::truthy;

    #[test]
    fn null_falsy() {
        assert!(!truthy(ValueRef::Null));
    }

    #[test]
    fn zero_falsy() {
        assert!(!truthy(ValueRef::Integer(0)))
    }

    #[test]
    fn one_truthy() {
        assert!(truthy(ValueRef::Integer(1)))
    }

    #[test]
    fn neg_one_truthy() {
        assert!(truthy(ValueRef::Integer(-1)))
    }

    #[test]
    fn real_zero_falsy() {
        assert!(!truthy(ValueRef::Real(0.0)))
    }

    #[test]
    fn real_neg_zero_falsy() {
        assert!(!truthy(ValueRef::Real(-0.0)))
    }

    #[test]
    fn real_truthy() {
        assert!(truthy(ValueRef::Real(1.0)))
    }

    #[test]
    fn nan_falsy() {
        assert!(!truthy(ValueRef::Real(f64::NAN)))
    }

    #[test]
    fn empty_str_falsy() {
        assert!(!truthy(ValueRef::Text(b"")))
    }

    #[test]
    fn nonempty_str_truthy() {
        assert!(truthy(ValueRef::Text(b"hello world")))
    }

    #[test]
    fn empty_blob_falsy() {
        assert!(!truthy(ValueRef::Blob(b"")))
    }

    #[test]
    fn nonempty_blob_truthy() {
        assert!(truthy(ValueRef::Blob(b"hello world")))
    }
}
