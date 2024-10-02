#![doc = include_str!("lib.md")]
#![allow(dead_code)]

use std::{borrow::Cow, io};

mod queries;
mod visit;

pub use visit::evaluate_template;

#[derive(Debug, thiserror::Error)]
pub enum Error {
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
    #[error("in invalid parameter: in element {0}, parameter {1}: has invalid format")]
    InvalidParameter(&'static str, String),
    #[error("in invalid parameter: in element {0}, query has parameter {1}, but there is no corresponding attribute")]
    MissingParameter(&'static str, String),

    #[error("SQL error: in query {0}: {1}")]
    Sql(String, rusqlite::Error),
    #[error("reserializing error: {0}")]
    Serialize(io::Error),
    #[error("html parsing error: {0}")]
    HtmlParse(Cow<'static, str>),
}

impl Error {
    /// Modify the element listed (in some errors).
    pub fn set_element(self, element: &'static str) -> Self {
        match self {
            Error::TemplateEval(_)
            | Error::Sql(_, _)
            | Error::Serialize(_)
            | Error::HtmlParse(_) => self,
            Error::MissingAttr(_, attr) => Error::MissingAttr(element, attr),
            Error::MissingQuery(_, a) => Error::MissingQuery(element, a),
            Error::Cardinality(_, a, b, c) => Error::Cardinality(element, a, b, c),
            Error::MissingColumn(_, a, b, c) => Error::MissingColumn(element, a, b, c),
            Error::NoDefaultColumn(_, a, b) => Error::NoDefaultColumn(element, a, b),
            Error::InvalidParameter(_, a) => Error::InvalidParameter(element, a),
            Error::MissingParameter(_, a) => Error::MissingParameter(element, a),
        }
    }
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
    use std::ops::Deref;

    use rusqlite::{params, Connection};
    use scraper::Html;
    use tempfile::NamedTempFile;

    pub const CCECKMAN_UUID: &str = "18adfb4d-6a38-4c81-b2e8-4d59e6467c9f";
    pub const OTHER_UUID: &str = "6de21789-6279-416c-9025-d090d407bc8c";

    /// Makes a test database and gets a connection to it.
    pub fn make_test_db() -> Connection {
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

    #[test]
    fn meta_conn_is_ro() {
        let conn = make_test_db();
        // Should fail: DB is read-only
        conn.execute("INSERT INTO users (uuid, name) VALUES (?, ?)", ["x", "y"])
            .unwrap_err();
    }

    /// Compare HTML for equal structure.
    pub fn html_equal(got: impl Deref<Target = str>, want: impl Deref<Target = str>) {
        let got = Html::parse_fragment(got.trim());
        let want = Html::parse_fragment(want.trim());
        assert_eq!(got, want);
    }
}
