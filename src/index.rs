// Copyright 2019 The Matrix.org Foundation CIC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::Path;
use tantivy as tv;

use crate::types::{EventId, RoomId};

#[cfg(test)]
use tempfile::TempDir;

pub(crate) struct Index {
    index: tv::Index,
    reader: tv::IndexReader,
    body_field: tv::schema::Field,
    topic_field: tv::schema::Field,
    name_field: tv::schema::Field,
    event_id_field: tv::schema::Field,
    room_id_field: tv::schema::Field,
    server_timestamp_field: tv::schema::Field,
}

pub(crate) struct Writer {
    pub(crate) inner: tv::IndexWriter,
    pub(crate) body_field: tv::schema::Field,
    pub(crate) event_id_field: tv::schema::Field,
    room_id_field: tv::schema::Field,
    server_timestamp_field: tv::schema::Field,
}

impl Writer {
    pub fn commit(&mut self) -> Result<(), tv::Error> {
        self.inner.commit()?;
        Ok(())
    }

    pub fn add_event(&mut self, body: &str, event_id: &str, room_id: &str, server_timestamp: u64) {
        let mut doc = tv::Document::default();
        doc.add_text(self.body_field, body);
        doc.add_text(self.event_id_field, event_id);
        doc.add_text(self.room_id_field, room_id);
        doc.add_u64(self.server_timestamp_field, server_timestamp);
        self.inner.add_document(doc);
    }
}

pub(crate) struct IndexSearcher {
    pub(crate) inner: tv::LeasedItem<tv::Searcher>,
    pub(crate) event_id_field: tv::schema::Field,
    pub(crate) server_timestamp_field: tv::schema::Field,
    pub(crate) query_parser: tv::query::QueryParser,
}

impl IndexSearcher {
    pub fn search(
        &self,
        term: &str,
        limit: usize,
        order_by_recent: bool,
        room_id: Option<&RoomId>,
    ) -> Vec<(f32, EventId)> {
        // TODO we might want to propagate those errors instead of returning
        // empty vectors.

        let term = if let Some(room) = room_id {
            format!("{} AND room_id:\"{}\"", term, room)
        } else {
            term.to_owned()
        };

        let query = match self.query_parser.parse_query(&term) {
            Ok(q) => q,
            Err(_e) => return vec![],
        };

        if order_by_recent {
            let collector = tv::collector::TopDocs::with_limit(limit);
            let collector = collector.order_by_u64_field(self.server_timestamp_field);

            let result = match self.inner.search(&query, &collector) {
                Ok(result) => result,
                Err(_e) => return vec![],
            };

            let mut docs = Vec::new();

            for (_, docaddress) in result {
                let doc = match self.inner.doc(docaddress) {
                    Ok(d) => d,
                    Err(_e) => continue,
                };

                let event_id: EventId = match doc.get_first(self.event_id_field) {
                    Some(s) => s.text().unwrap().to_owned(),
                    None => continue,
                };

                docs.push((1.0, event_id));
            }
            docs
        } else {
            let result = match self
                .inner
                .search(&query, &tv::collector::TopDocs::with_limit(limit))
            {
                Ok(result) => result,
                Err(_e) => return vec![],
            };

            let mut docs = Vec::new();

            for (score, docaddress) in result {
                let doc = match self.inner.doc(docaddress) {
                    Ok(d) => d,
                    Err(_e) => continue,
                };

                let event_id: EventId = match doc.get_first(self.event_id_field) {
                    Some(s) => s.text().unwrap().to_owned(),
                    None => continue,
                };

                docs.push((score, event_id));
            }
            docs
        }
    }
}

impl Index {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Index, tv::Error> {
        let mut schemabuilder = tv::schema::Schema::builder();

        let body_field = schemabuilder.add_text_field("body", tv::schema::TEXT);
        let topic_field = schemabuilder.add_text_field("topic", tv::schema::TEXT);
        let name_field = schemabuilder.add_text_field("name", tv::schema::TEXT);
        let room_id_field = schemabuilder.add_text_field("room_id", tv::schema::TEXT);
        let server_timestamp_field =
            schemabuilder.add_u64_field("server_timestamp", tv::schema::FAST);

        let event_id_field = schemabuilder.add_text_field("event_id", tv::schema::STORED);

        let schema = schemabuilder.build();

        let index_dir = tv::directory::MmapDirectory::open(path)?;

        let index = tv::Index::open_or_create(index_dir, schema)?;
        let reader = index.reader()?;

        Ok(Index {
            index,
            reader,
            body_field,
            topic_field,
            name_field,
            event_id_field,
            room_id_field,
            server_timestamp_field,
        })
    }

    pub fn get_searcher(&self) -> IndexSearcher {
        let searcher = self.reader.searcher();
        let query_parser = tv::query::QueryParser::for_index(
            &self.index,
            vec![
                self.body_field,
                self.topic_field,
                self.name_field,
                self.room_id_field,
            ],
        );

        IndexSearcher {
            inner: searcher,
            query_parser,
            event_id_field: self.event_id_field,
            server_timestamp_field: self.server_timestamp_field,
        }
    }

    pub fn reload(&self) -> Result<(), tv::Error> {
        self.reader.reload()
    }

    pub fn get_writer(&self) -> Result<Writer, tv::Error> {
        Ok(Writer {
            inner: self.index.writer(50_000_000)?,
            body_field: self.body_field,
            event_id_field: self.event_id_field,
            room_id_field: self.room_id_field,
            server_timestamp_field: self.server_timestamp_field,
        })
    }
}

#[test]
fn add_an_event() {
    let tmpdir = TempDir::new().unwrap();
    let index = Index::new(&tmpdir).unwrap();

    let event_id = "$15163622445EBvZJ:localhost";
    let mut writer = index.get_writer().unwrap();

    writer.add_event("Test message", &event_id, "!Test:room", 1516362244026);
    writer.commit().unwrap();
    index.reload().unwrap();

    let searcher = index.get_searcher();
    let result = searcher.search("Test", 10, false, None);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1, event_id)
}

#[test]
fn add_events_to_differing_rooms() {
    let tmpdir = TempDir::new().unwrap();
    let index = Index::new(&tmpdir).unwrap();

    let event_id = "$15163622445EBvZJ:localhost";
    let mut writer = index.get_writer().unwrap();

    writer.add_event("Test message", &event_id, "!Test:room", 1516362244026);
    writer.add_event(
        "Test message",
        "$16678900:localhost",
        "!Test2:room",
        1516362244026,
    );

    writer.commit().unwrap();
    index.reload().unwrap();

    let searcher = index.get_searcher();
    let result = searcher.search("Test", 10, false, Some(&"!Test:room".to_string()));

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1, event_id);

    let result = searcher.search("Test", 10, false, None);
    assert_eq!(result.len(), 2);
}

#[test]
fn order_results_by_date() {
    let tmpdir = TempDir::new().unwrap();
    let index = Index::new(&tmpdir).unwrap();

    let event_id = "$15163622445EBvZJ:localhost";
    let mut writer = index.get_writer().unwrap();

    writer.add_event("Test message", &event_id, "!Test:room", 1516362244026);
    writer.add_event(
        "Test message",
        "$16678900:localhost",
        "!Test2:room",
        1516362244027,
    );

    writer.commit().unwrap();
    index.reload().unwrap();

    let searcher = index.get_searcher();
    let result = searcher.search("Test", 10, true, None);

    assert_eq!(result.len(), 2);
    assert_eq!(result[1].1, event_id);
}
