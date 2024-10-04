#![cfg(test)]

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
( id INTEGER PRIMARY KEY NOT NULL
, uuid TEXT NOT NULL
, name TEXT NOT NULL
, UNIQUE(uuid)
, UNIQUE(name)
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
    let got_html = Html::parse_fragment(got.trim());
    let want_html = Html::parse_fragment(want.trim());
    assert_eq!(
        got_html,
        want_html,
        "got:\n---\n{}\n---\nwant:\n---\n{}\n---\n",
        got.trim(),
        want.trim()
    );
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
        <htmpl-insert query="q"></htmpl-insert>
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
        <htmpl-insert query="q"></htmpl-insert>
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
        <htmpl-insert query="q(does-not-exist)"></htmpl-insert>
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
        <htmpl-insert query="q(uuid)"></htmpl-insert>
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
        <htmpl-insert query="q"></htmpl-insert>
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
        <htmpl-insert query="q"></htmpl-insert>
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
                <htmpl-insert query="q"></htmpl-insert>
            </div>
            <htmpl-insert query="q"></htmpl-insert>
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

#[test_log::test]
fn foreach_multiple() {
    let db = make_test_db();
    const TEMPLATE: &str = r#"
        <htmpl-query name="q">
            SELECT * FROM users;
        </htmpl-query>
        <htmpl-foreach query="q">
            <htmpl-insert query="q(uuid)"></htmpl-insert> <htmpl-insert query="q(name)"></htmpl-insert>
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
            <htmpl-insert query="q(uuid)"></htmpl-insert> <htmpl-insert query="q(name)"></htmpl-insert>
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
        <htmpl-query name="get_uuid">
            SELECT uuid FROM users;
        </htmpl-query>
        <htmpl-foreach query="get_uuid">
            <htmpl-query name="get_name" :uuid="get_uuid(uuid)">
                SELECT name FROM users WHERE uuid = :uuid
            </htmpl-query>
            <htmpl-insert query="get_name"></htmpl-insert>
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
        <htmpl-insert query="admin_uuid"></htmpl-insert>
        "#;
    let result = evaluate_template(TEMPLATE, &db).expect("unexpected error");
    html_equal(result, CCECKMAN_UUID);
}

#[test_log::test]
fn single_attr() {
    let conn = make_test_db();
    const TEMPLATE: &str = r#"
        <htmpl-query name="q">SELECT name, (uuid || " name") AS uuid_class FROM users ORDER BY name ASC LIMIT
            1;</htmpl-query>
        <htmpl-attr select=".name" query="q(uuid_class)" attr="class"></htmpl-attr>
        <div class="name"><htmpl-insert query="q(name)"></htmpl-insert></div>
        "#;
    let result = evaluate_template(TEMPLATE, &conn).unwrap();
    html_equal(
        result,
        r#"<div class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name">cceckman</div>"#,
    );
}

#[test_log::test]
fn attr_selector() {
    let conn = make_test_db();
    const TEMPLATE: &str = r#"
        <htmpl-query name="q">SELECT name, (uuid || " name") AS uuid_class FROM users ORDER BY name ASC LIMIT
            1;</htmpl-query>
        <htmpl-attr select=".name" query="q(uuid_class)" attr="class"></htmpl-attr>
        <div class="name"><a href="https://cceckman.com"><htmpl-insert query="q(name)"></htmpl-insert></a></div>
        <div><a class="name" href="https://cceckman.com"><htmpl-insert query="q(name)"></htmpl-insert></a></div>"#;
    let result = evaluate_template(TEMPLATE, &conn).unwrap();
    // Don't depend on attribute order:
    html_equal(
        result,
        r#"
        <div class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name"><a href="https://cceckman.com">cceckman</a></div>
        <div><a href="https://cceckman.com" class="18adfb4d-6a38-4c81-b2e8-4d59e6467c9f name">cceckman</a></div>
        "#.trim(),
        );
}

#[test_log::test]
fn invalid_html() {
    let conn = make_test_db();
    const TEMPLATE: &str = r#"<html></div>"#;
    evaluate_template(TEMPLATE, &conn).expect_err("accepted invalid HTML structure");
}

#[test_log::test]
fn truthy() {
    let conn = make_test_db();
    let template: String = format!(
        r#"
        <htmpl-query name="q">SELECT name, uuid = "{}" AS is_charles FROM users;</htmpl-query>
        <htmpl-foreach query="q"><htmpl-insert query="q(name)"></htmpl-insert><htmpl-if true="q(is_charles)"> (hi Charles!)</htmpl-if>
        </htmpl-foreach>
        "#,
        CCECKMAN_UUID
    );
    let result = evaluate_template(template, &conn).unwrap();
    html_equal(
        result,
        r#"
        cceckman (hi Charles!)
        ddedkman
    "#,
    );
}

#[test_log::test]
fn falsy() {
    let conn = make_test_db();
    let template: String = format!(
        r#"
        <htmpl-query name="q">SELECT name, uuid = "{}" AS is_charles FROM users;</htmpl-query>
        <htmpl-foreach query="q"><htmpl-insert query="q(name)"></htmpl-insert><htmpl-if false="q(is_charles)"> (who are you?)</htmpl-if>
        </htmpl-foreach>
        "#,
        CCECKMAN_UUID
    );
    let result = evaluate_template(template, &conn).unwrap();
    html_equal(
        result,
        r#"
        cceckman
        ddedkman (who are you?)
    "#,
    );
}

#[test_log::test]
fn empty_is_falsy() {
    let conn = make_test_db();
    const TEMPLATE: &str = r#"
        <htmpl-query name="q">SELECT name FROM users WHERE name = "odysseus";</htmpl-query>
        <htmpl-if false="q(name)">No one is here</htmpl-if>
        "#;
    let result = evaluate_template(TEMPLATE, &conn).unwrap();
    html_equal(result, "No one is here");
}
