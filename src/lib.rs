#![allow(dead_code)]
use std::ops::Deref;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("failed at template evaluation: {0}")]
    TemplateEval(String),
}

impl<T> From<Error> for Result<T, Error> {
    fn from(val: Error) -> Self {
        Err(val)
    }
}

/// Evaluate the provided template.
///
/// Can draw data from the provided database.
fn evaluate(
    _template: &str,
    _db: impl Deref<Target = rusqlite::Connection>,
) -> Result<String, Error> {
    Error::TemplateEval("not implemented".to_owned()).into()
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use uuid::Uuid;

    const CCECKMAN_UUID: &str = "18adfb4d-6a38-4c81-b2e8-4d59e6467c9f";

    fn make_test_db() -> rusqlite::Result<Connection> {
        let conn = rusqlite::Connection::open_in_memory()?;
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
            params![],
        )?;

        let uuid: Uuid = CCECKMAN_UUID.parse().expect("invalid uuid");
        conn.execute(
            r#"INSERT INTO users (uuid, name) VALUES (?, ?)"#,
            params![uuid.to_string(), "cceckman"],
        )?;
        Ok(conn)
    }

    #[test]
    fn insert_single_value() {
        let db = make_test_db().expect("could not get test DB");
        const TEMPLATE: &str = r#"
<htmpl-query name="q">
SELECT uuid FROM users WHERE name = "cceckman" LIMIT 1;
</htmpl-query>
<htmpl-insert from="q" name="uuid" />
        "#;
        let result: String = crate::evaluate(TEMPLATE, &db).expect("failed to evaluate template");
        assert_eq!(result.trim(), CCECKMAN_UUID);
    }
}
