use std::ops::Deref;

use crate::{evaluate_template, Error};
use rusqlite::{params, Connection};
use scraper::Html;
use tempfile::NamedTempFile;

const CCECKMAN_UUID: &str = "18adfb4d-6a38-4c81-b2e8-4d59e6467c9f";
const OTHER_UUID: &str = "6de21789-6279-416c-9025-d090d407bc8c";

/// Makes a test database and gets a connection to it.
fn make_test_db() -> Connection {
    let dbfile = NamedTempFile::new().expect("could not create temp DB");
    let conn = Connection::open(format!("file:{}?mode=rwc", dbfile.path().display()))
        .expect("failed to create test DB");
    conn.execute(
        r#"
    CREATE TABLE users
    (   id      INTEGER PRIMARY KEY NOT NULL
    ,   uuid    TEXT NOT NULL
    ,   name    TEXT NOT NULL
    ,   UNIQUE(uuid)
    ,   UNIQUE(name)
    );
                "#,
        [],
    )
    .expect("failed to prepare test DB schema");
    conn.execute(
        r#"INSERT INTO users (uuid, name) VALUES (?, ?), (?, ?)"#,
        params![CCECKMAN_UUID, "cceckman", OTHER_UUID, "ddedkman"],
    )
    .expect("failed to prepare test DB content");
    // We keep the writing "conn" around until we've re-opened it as read-only.
    // sqlite appears to forget the database unless there is some reference to
    // it.

    let ro = Connection::open(format!("file:{}?mode=ro", dbfile.path().display()))
        .expect("failed to re-open test DB");

    dbfile.keep().unwrap();
    ro
}

/// Compare HTML for equal structure.
fn html_equal(got: impl Deref<Target = str>, want: impl Deref<Target = str>) {
    let got = Html::parse_fragment(got.trim());
    let want = Html::parse_fragment(want.trim());
    assert_eq!(got, want);
}

#[test]
fn meta_conn_is_ro() {
    let conn = make_test_db();
    // Should fail: DB is read-only
    conn.execute("INSERT INTO users (uuid, name) VALUES (?, ?)", ["x", "y"])
        .unwrap_err();
}

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
    html_equal(result, CCECKMAN_UUID);
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
    html_equal(result, CCECKMAN_UUID);
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
    html_equal(
        trimmed,
        format!("<div><div>{}</div>{}</div>", CCECKMAN_UUID, OTHER_UUID),
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
    html_equal(result, CCECKMAN_UUID);
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
    html_equal(
        result,
        r#"<div class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name">cceckman</div>"#,
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
