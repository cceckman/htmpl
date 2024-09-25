#![allow(dead_code)]

use std::io;

mod visit;

#[derive(Debug, thiserror::Error)]
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

    #[error("SQL error: in query {0}: {1}")]
    Sql(String, rusqlite::Error),
    #[error("Reserializing error: ")]
    Serialize(io::Error),
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::TemplateEval(l0), Self::TemplateEval(r0)) => l0 == r0,
            (Self::MissingAttr(l0, l1), Self::MissingAttr(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::MissingQuery(l0, l1), Self::MissingQuery(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::Cardinality(l0, l1, l2, l3), Self::Cardinality(r0, r1, r2, r3)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3
            }
            (Self::MissingColumn(l0, l1, l2, l3), Self::MissingColumn(r0, r1, r2, r3)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3
            }
            (Self::NoDefaultColumn(l0, l1, l2), Self::NoDefaultColumn(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::Sql(l0, l1), Self::Sql(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::Serialize(l0), Self::Serialize(r0)) => {
                (l0.kind() == r0.kind()) && l0.to_string() == r0.to_string()
            }
            _ => false,
        }
    }
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
