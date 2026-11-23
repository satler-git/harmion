//! Local full-text index. The authoritative store remains JSON/in-memory; this
//! SQLite FTS5 index is rebuilt lazily and invalidated on every mutation.
//!
//! FTS5 trigram handles Japanese and substring search. Very short queries
//! (1–2 scalars) fall back to a normalized LIKE scan, because a trigram cannot
//! match them.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use unicode_normalization::UnicodeNormalization;

use crate::model::Content;

pub struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    pub fn build(contents: &BTreeMap<String, Content>) -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE docs USING fts5(
                id UNINDEXED, title, body, tags, tokenize='trigram'
            );",
        )?;
        let index = Self { conn };
        for content in contents.values() {
            index.upsert(content)?;
        }
        Ok(index)
    }

    fn upsert(&self, content: &Content) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO docs(id, title, body, tags) VALUES (?1, ?2, ?3, ?4)",
            params![
                content.id,
                normalize(&content.title),
                normalize(&content.body),
                normalize(&content.tags.join(" ")),
            ],
        )?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
        let (text, tag) = split_query(query);
        let text = normalize(&text);
        if text.is_empty() && tag.is_none() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        if !text.is_empty() && text.chars().count() >= 3 {
            let phrase = format!("\"{}\"", text.replace('"', "\"\""));
            let mut statement = self
                .conn
                .prepare("SELECT id FROM docs WHERE docs MATCH ?1 ORDER BY rank LIMIT ?2")?;
            ids = statement
                .query_map(params![phrase, limit as i64], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
        }
        if ids.is_empty() {
            let needle = format!("%{}%", text);
            let mut statement = self.conn.prepare(
                "SELECT id FROM docs
                 WHERE (?1 = '%%' OR title LIKE ?1 OR body LIKE ?1 OR tags LIKE ?1)
                 ORDER BY id LIMIT ?2",
            )?;
            ids = statement
                .query_map(params![needle, limit as i64], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
        }
        if let Some(tag) = tag {
            let tag = normalize(&tag);
            ids.retain(|id| {
                self.conn
                    .query_row("SELECT tags FROM docs WHERE id = ?1", params![id], |row| {
                        row.get::<_, String>(0)
                    })
                    .map(|tags| tags.split_whitespace().any(|candidate| candidate == tag))
                    .unwrap_or(false)
            });
        }
        ids.truncate(limit);
        Ok(ids)
    }
}

fn split_query(query: &str) -> (String, Option<String>) {
    let mut words = Vec::new();
    let mut tag = None;
    for word in query.split_whitespace() {
        if let Some(value) = word.strip_prefix("tag:") {
            tag = Some(value.to_owned());
        } else {
            words.push(word);
        }
    }
    (words.join(" "), tag)
}

fn normalize(value: &str) -> String {
    value.nfkc().collect::<String>().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Content;

    fn contents() -> BTreeMap<String, Content> {
        let mut contents = BTreeMap::new();
        let mut add = |id: &str, title: &str, body: &str, tags: &[&str]| {
            let mut content = Content::fixture(id, "page", title, body.to_owned());
            content.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
            contents.insert(id.to_owned(), content);
        };
        add(
            "a",
            "家計の記録",
            "毎月の支出を記録する。",
            &["家計", "ledger"],
        );
        add("b", "電気代", "電気代のメモ。", &["固定費", "家計"]);
        add("c", "Rustのメモ", "所有権と借用。", &["Rust"]);
        contents
    }

    #[test]
    fn finds_japanese_and_tags() {
        let index = SearchIndex::build(&contents()).unwrap();
        assert_eq!(index.search("電気", 10).unwrap(), ["b"]);
        assert_eq!(index.search("買い物", 10).unwrap(), Vec::<String>::new());
        assert_eq!(index.search("tag:家計 電気", 10).unwrap(), ["b"]);
        assert_eq!(index.search("Rust", 10).unwrap(), ["c"]);
    }

    #[test]
    fn ten_thousand_pages_search_is_bounded() {
        let mut contents = BTreeMap::new();
        for index in 0..10_000 {
            let id = format!("{index:064x}");
            let mut content = Content::fixture(
                &id,
                "page",
                &format!("ページ {index}"),
                format!("本文 {index} 電気代 固定費 家計\n"),
            );
            content.tags = vec!["家計".to_owned()];
            contents.insert(id, content);
        }
        let started = std::time::Instant::now();
        let index = SearchIndex::build(&contents).unwrap();
        let hits = index.search("電気", 20).unwrap();
        assert!(!hits.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_millis(1_500));
    }
}
