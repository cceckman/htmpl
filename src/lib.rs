#![allow(dead_code)]
use std::ops::Deref;

mod visit;
use scraper::Html;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum Error {
    #[error("failed at template evaluation: {0}")]
    TemplateEval(String),
    #[error("missing attribute: from element {0}, attribute {1}")]
    MissingAttr(&'static str, &'static str),
    #[error("missing query: from element {0}, query {1} is not in scope")]
    MissingQuery(&'static str, String),
    #[error("incorrect cardinality: from element {0}, query {1} returned {2} rows, wanted {3}")]
    Cardinality(&'static str, String, usize, usize),
    #[error("invalid column: from element {0}, query {1} has columns {2}, wanted {3}")]
    MissingColumn(&'static str, String, String, String),
    #[error("invalid column: from element {0}, query {1} has columns {2}, wanted one column")]
    NoDefaultColumn(&'static str, String, String),
}

impl<T> From<Error> for Result<T, Error> {
    fn from(val: Error) -> Self {
        Err(val)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use uuid::Uuid;

    pub const CCECKMAN_UUID: &str = "18adfb4d-6a38-4c81-b2e8-4d59e6467c9f";

    pub fn make_test_db() -> Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("failed to create test DB");
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
        )
        .expect("failed to prepare test DB schema");

        let uuid: Uuid = CCECKMAN_UUID.parse().expect("invalid uuid");
        conn.execute(
            r#"INSERT INTO users (uuid, name) VALUES (?, ?)"#,
            params![uuid.to_string(), "cceckman"],
        )
        .expect("failed to prepare test DB content");
        conn
    }
}
