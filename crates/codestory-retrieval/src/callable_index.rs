//! Immutable core-generation callable postings. Source evidence stays file-addressed.
mod mention_candidates;
use anyhow::{Result, bail};
use codestory_contracts::graph::NodeId;
use codestory_workspace::paths::sqlite_open_path;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

pub const CALLABLE_INDEX_FILE: &str = "callable-lexical.sqlite3";
const SCHEMA_VERSION: i32 = 2;
const MAX_QUERY_BYTES: usize = 16_384;
const MAX_QUERY_TERMS: usize = 256;
const MAX_QUERY_POSTINGS: usize = 1_500_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub indexed_callables: usize,
    pub partial_files: usize,
    pub omitted_files: usize,
    pub omitted_definitions: usize,
}

#[derive(Debug)]
pub struct Definition<'a> {
    pub node_id: NodeId,
    pub file_id: i64,
    pub name: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    pub body_sha256: &'a str,
}

#[derive(Debug)]
pub struct Hit {
    pub node_id: NodeId,
    pub file_id: i64,
    pub path: String,
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    pub file_sha256: String,
    pub body_sha256: String,
    pub file_bytes: u64,
    pub score: f64,
}

pub struct Writer<'a> {
    tx: &'a Transaction<'a>,
    vocabulary: HashMap<String, (i64, usize)>,
    count: usize,
    term_count: usize,
    next_doc: i64,
    next_term: i64,
    inherited: bool,
}

impl Writer<'_> {
    pub fn file(&self, id: i64, path: &str, sha256: &str, bytes: usize) -> Result<()> {
        self.tx
            .prepare_cached("INSERT INTO source VALUES (?1,?2,?3,?4)")?
            .execute(params![id, path, sha256, i64::try_from(bytes)?])?;
        Ok(())
    }

    pub fn file_status(
        &self,
        id: i64,
        partial: bool,
        omitted: bool,
        invalid_definitions: usize,
    ) -> Result<()> {
        self.tx
            .prepare_cached("INSERT INTO file_status VALUES (?1,?2,?3,?4)")?
            .execute(params![
                id,
                partial,
                omitted,
                i64::try_from(invalid_definitions)?
            ])?;
        Ok(())
    }

    pub fn definition(&mut self, location: Definition<'_>, path: &str, body: &str) -> Result<()> {
        let name = location
            .name
            .rsplit(['.', ':'])
            .next()
            .unwrap_or(location.name);
        let mut counts = BTreeMap::new();
        for word in code_terms(path).chain(code_terms(body)) {
            *counts.entry(word).or_insert(0_usize) += 1;
        }
        for word in code_terms(name) {
            *counts.entry(word).or_insert(0_usize) += 2;
        }
        let length = counts.values().sum::<usize>();
        let doc_id = self.next_doc;
        self.next_doc += 1;
        self.count += 1;
        self.term_count += length;
        self.tx
            .prepare_cached("INSERT INTO document VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?
            .execute(params![
                doc_id,
                location.file_id,
                location.node_id.0,
                location.name,
                location.start_line,
                location.end_line,
                location.start_byte,
                location.end_byte,
                location.body_sha256,
                i64::try_from(length)?
            ])?;
        let mut write = self
            .tx
            .prepare_cached("INSERT INTO posting VALUES (?1,?2,?3)")?;
        let mut existing = self
            .tx
            .prepare_cached("SELECT id,documents FROM term WHERE name=?1")?;
        for (token, frequency) in counts {
            let (id, df) = match self.vocabulary.entry(token) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let inherited = if self.inherited {
                        existing
                            .query_row([entry.key().as_str()], |row| {
                                Ok((row.get(0)?, read_size(row, 1)?))
                            })
                            .optional()?
                    } else {
                        None
                    };
                    entry.insert(inherited.unwrap_or_else(|| {
                        let id = self.next_term;
                        self.next_term += 1;
                        (id, 0)
                    }))
                }
            };
            *df += 1;
            write.execute(params![*id, doc_id, i64::try_from(frequency)?])?;
        }
        Ok(())
    }

    fn remove_file(&mut self, id: i64, cancelled: &impl Fn() -> bool) -> Result<()> {
        let (count, length) = self.tx.query_row(
            "SELECT count(*), coalesce(sum(length),0) FROM document WHERE file_id=?1",
            [id],
            |row| Ok((read_size(row, 0)?, read_size(row, 1)?)),
        )?;
        self.count = self
            .count
            .checked_sub(count)
            .ok_or_else(|| anyhow::anyhow!("callable document count is inconsistent"))?;
        self.term_count = self
            .term_count
            .checked_sub(length)
            .ok_or_else(|| anyhow::anyhow!("callable term count is inconsistent"))?;
        let mut terms = self.tx.prepare_cached(
            "SELECT p.term_id,count(*) FROM document d
            JOIN posting p ON p.doc_id=d.id WHERE d.file_id=?1 GROUP BY p.term_id",
        )?;
        let mut read = terms.query([id])?;
        let mut update = self.tx.prepare_cached(
            "UPDATE term SET documents=documents-?2 WHERE id=?1 RETURNING documents",
        )?;
        let mut prune = self.tx.prepare_cached("DELETE FROM term WHERE id=?1")?;
        while let Some(row) = read.next()? {
            if cancelled() {
                bail!("callable index build cancelled");
            }
            let term = row.get::<_, i64>(0)?;
            let removed = row.get::<_, i64>(1)?;
            let remaining = update.query_row(params![term, removed], |r| read_size(r, 0))?;
            if remaining == 0 {
                prune.execute([term])?;
            }
        }
        self.tx.execute(
            "DELETE FROM posting WHERE doc_id IN (SELECT id FROM document WHERE file_id=?1)",
            [id],
        )?;
        self.tx
            .execute("DELETE FROM document WHERE file_id=?1", [id])?;
        self.tx.execute("DELETE FROM source WHERE id=?1", [id])?;
        self.tx
            .execute("DELETE FROM file_status WHERE id=?1", [id])?;
        Ok(())
    }
}

/// A predecessor whose complete change scope is owned by the caller.
pub struct PreviousGeneration<'a> {
    pub path: &'a Path,
    pub generation: &'a str,
    pub run: &'a str,
    pub changed_file_ids: &'a [i64],
}

/// Stage and seal a generation-local artifact. The caller owns the publication fence.
pub fn write(
    destination: &Path,
    generation: &str,
    run: &str,
    previous: Option<PreviousGeneration<'_>>,
    populate: impl FnOnce(&mut Writer<'_>) -> Result<()>,
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    let temporary = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let inherited = if let Some(previous) = &previous {
            let reader = open(previous.path, previous.generation, previous.run)?;
            let facts = (reader.coverage.indexed_callables, reader.total_terms);
            drop(reader);
            crate::copy_on_write::stage_file(previous.path, &temporary, &cancelled)?;
            crate::copy_on_write::make_file_owner_writable(&temporary)?;
            Some(facts)
        } else {
            None
        };
        let mut connection = Connection::open(sqlite_open_path(&temporary))?;
        let tx = connection.transaction()?;
        if inherited.is_none() {
            create_schema(&tx)?;
        }
        let mut writer = Writer {
            tx: &tx,
            vocabulary: HashMap::new(),
            count: inherited.map_or(0, |(count, _)| count),
            term_count: inherited.map_or(0, |(_, terms)| terms),
            next_doc: tx.query_row("SELECT coalesce(max(id),0)+1 FROM document", [], |row| {
                row.get(0)
            })?,
            next_term: tx.query_row("SELECT coalesce(max(id),0)+1 FROM term", [], |row| {
                row.get(0)
            })?,
            inherited: inherited.is_some(),
        };
        if let Some(previous) = previous {
            for &file in previous.changed_file_ids {
                writer.remove_file(file, &cancelled)?;
            }
            tx.execute("DELETE FROM meta", [])?;
        }
        populate(&mut writer)?;
        {
            let mut write = tx.prepare(
                "INSERT INTO term VALUES (?1,?2,?3)
                ON CONFLICT(id) DO UPDATE SET documents=excluded.documents",
            )?;
            for (index, (term, (id, df))) in writer.vocabulary.into_iter().enumerate() {
                if index % 256 == 0 && cancelled() {
                    bail!("callable index build cancelled");
                }
                write.execute(params![id, term, i64::try_from(df)?])?;
            }
        }
        tx.execute("INSERT INTO meta SELECT ?1,?2,?3,?4,?5,
                    coalesce(sum(partial),0),coalesce(sum(omitted),0),coalesce(sum(invalid_definitions),0) FROM file_status",
                   params![SCHEMA_VERSION,generation,run,i64::try_from(writer.count)?,i64::try_from(writer.term_count)?])?;
        tx.commit()?;
        connection.close().map_err(|(_, error)| error)?;
        if cancelled() {
            bail!("callable index build cancelled");
        }
        crate::copy_on_write::publish_immutable_file_atomic(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn create_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE meta(version INTEGER NOT NULL, generation TEXT NOT NULL, run TEXT NOT NULL,
                         documents INTEGER NOT NULL, terms INTEGER NOT NULL, partial_files INTEGER NOT NULL,
                         omitted_files INTEGER NOT NULL, omitted_definitions INTEGER NOT NULL);
                     CREATE TABLE source(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,checksum TEXT NOT NULL,bytes INTEGER NOT NULL);
                     CREATE TABLE file_status(id INTEGER PRIMARY KEY,partial INTEGER NOT NULL,omitted INTEGER NOT NULL,invalid_definitions INTEGER NOT NULL);
                     CREATE TABLE document(id INTEGER PRIMARY KEY,file_id INTEGER NOT NULL,node_id INTEGER NOT NULL,
                         name TEXT NOT NULL,start_line INTEGER NOT NULL,end_line INTEGER NOT NULL,start_byte INTEGER NOT NULL,
                         end_byte INTEGER NOT NULL,checksum TEXT NOT NULL,length INTEGER NOT NULL);
                     CREATE INDEX document_by_file ON document(file_id);
                     CREATE TABLE term(id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,documents INTEGER NOT NULL);
                     CREATE TABLE posting(term_id INTEGER NOT NULL,doc_id INTEGER NOT NULL,
                         frequency INTEGER NOT NULL,PRIMARY KEY(term_id,doc_id)) WITHOUT ROWID;
                     CREATE INDEX posting_by_document ON posting(doc_id);")?;
    Ok(())
}

pub fn open(path: &Path, generation: &str, run: &str) -> Result<Reader> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        bail!("callable index must be a regular file");
    }
    let connection = Connection::open_with_flags(
        sqlite_open_path(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    let (version, stored_generation, stored_run, total_terms, coverage) = connection.query_row(
        "SELECT version,generation,run,terms,documents,partial_files,omitted_files,omitted_definitions FROM meta",
        [], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, read_size(row, 3)?, Coverage {
            indexed_callables: read_size(row, 4)?, partial_files: read_size(row, 5)?, omitted_files: read_size(row, 6)?,
            omitted_definitions: read_size(row, 7)?,
        })),
    )?;
    if version != SCHEMA_VERSION || stored_generation != generation || stored_run != run {
        bail!("callable index does not belong to the selected core generation");
    }
    // Prepare without reading postings so an incomplete cache can be rebuilt on the next index.
    connection.prepare(
        "SELECT s.id,s.path,s.checksum,s.bytes,d.id,d.node_id,d.name,d.start_line,d.end_line,
                d.start_byte,d.end_byte,d.checksum,d.length,t.id,t.name,t.documents,p.frequency,
                f.partial,f.omitted,f.invalid_definitions
         FROM posting p JOIN term t ON t.id=p.term_id JOIN document d ON d.id=p.doc_id
         JOIN source s ON s.id=d.file_id JOIN file_status f ON f.id=s.id LIMIT 0",
    )?;
    Ok(Reader {
        connection,
        total_terms,
        coverage,
    })
}

fn read_size(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    let value = row.get::<_, i64>(column)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

pub struct Reader {
    connection: Connection,
    total_terms: usize,
    pub coverage: Coverage,
}

struct RankedDocument {
    path: Arc<str>,
    start: u32,
    end: u32,
    score: f64,
}

#[derive(Debug, Default)]
pub struct RankedResult {
    pub hits: Vec<Hit>,
    pub truncated: bool,
    pub has_more: bool,
    pub absent_terms: Vec<String>,
}

impl Reader {
    /// Rank only the documents admitted by the path scope before selecting the result cutoff.
    /// The budget bounds the actual postings touched, including excluded paths.
    pub fn rank(
        &self,
        question: &str,
        limit: usize,
        in_scope: impl Fn(&str) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<RankedResult> {
        let mut result = RankedResult::default();
        let count = self.coverage.indexed_callables;
        if limit == 0 || count == 0 || self.total_terms == 0 {
            return Ok(result);
        }
        let question_length = question.len().min(MAX_QUERY_BYTES);
        let question_end = question.floor_char_boundary(question_length);
        result.truncated = question_end < question.len();
        let mut terms = code_terms(&question[..question_end]).collect::<Vec<_>>();
        terms.sort_unstable();
        terms.dedup();
        result.truncated |= terms.len() > MAX_QUERY_TERMS;
        terms.truncate(MAX_QUERY_TERMS);
        let average = self.total_terms as f64 / count as f64;
        let mut lookup = self
            .connection
            .prepare("SELECT id,documents FROM term WHERE name=?1")?;
        let mut terms_with_postings = Vec::with_capacity(terms.len());
        for term in terms {
            let pair = lookup
                .query_row([term.as_str()], |row| {
                    Ok((row.get::<_, i64>(0)?, read_size(row, 1)?))
                })
                .optional()?;
            if let Some((id, df)) = pair {
                if df == 0 || df > count {
                    bail!("invalid callable index term frequency");
                }
                terms_with_postings.push((df, term, id));
            } else if term.len() >= 3 {
                result.absent_terms.push(term);
            }
        }
        // Preserve the frozen scorer's term order. Only when all postings
        // exceed the budget, prefer rarer whole lists and disclose the cutoff.
        if terms_with_postings
            .iter()
            .fold(0_usize, |count, (df, _, _)| count.saturating_add(*df))
            > MAX_QUERY_POSTINGS
        {
            terms_with_postings.sort_unstable();
            let mut remaining = MAX_QUERY_POSTINGS;
            terms_with_postings.retain(|(df, _, _)| {
                if *df > remaining {
                    result.truncated = true;
                    false
                } else {
                    remaining -= *df;
                    true
                }
            });
        }
        terms_with_postings.sort_unstable_by(|a, b| a.1.cmp(&b.1));
        let mut postings = self.connection.prepare(
            "SELECT p.doc_id,d.length,p.frequency,d.file_id,s.path,d.start_line,d.end_line
             FROM posting p JOIN document d ON d.id=p.doc_id JOIN source s ON s.id=d.file_id
             WHERE p.term_id=?1 ORDER BY p.doc_id",
        )?;
        let mut scoped = HashMap::<i64, Option<Arc<str>>>::new();
        let mut scores = HashMap::<i64, RankedDocument>::new();
        for (df, _, id) in terms_with_postings {
            if cancelled() {
                bail!("callable query cancelled");
            }
            let idf = ((count - df) as f64 + 0.5) / (df as f64 + 0.5);
            let idf = idf.ln_1p();
            let mut read = postings.query([id])?;
            let mut read_count = 0;
            while let Some(row) = read.next()? {
                read_count += 1;
                if read_count > df {
                    bail!("callable index postings do not match term metadata");
                }
                if read_count % 256 == 0 && cancelled() {
                    bail!("callable query cancelled");
                }
                let file_id = row.get::<_, i64>(3)?;
                if let std::collections::hash_map::Entry::Vacant(entry) = scoped.entry(file_id) {
                    let value = row.get_ref(4)?.as_str()?;
                    entry.insert(in_scope(value).then(|| Arc::from(value)));
                }
                let Some(path) = &scoped[&file_id] else {
                    continue;
                };
                let document = row.get::<_, i64>(0)?;
                let length = read_size(row, 1)?;
                let frequency = read_size(row, 2)?;
                if frequency == 0 || frequency > length {
                    bail!("invalid callable index posting");
                }
                let entry = match scores.entry(document) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(RankedDocument {
                            path: path.clone(),
                            start: row.get(5)?,
                            end: row.get(6)?,
                            score: 0.0,
                        })
                    }
                };
                let frequency = frequency as f64;
                entry.score += idf * frequency * 2.2
                    / (frequency + 1.2 * (0.25 + 0.75 * length as f64 / average));
            }
            if read_count != df {
                bail!("callable index postings do not match term metadata");
            }
        }
        let mut choices = scores.into_iter().collect::<Vec<_>>();
        let compare = |(id, a): &(i64, RankedDocument), (other, b): &(i64, RankedDocument)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| (&a.path, a.start, a.end, id).cmp(&(&b.path, b.start, b.end, other)))
        };
        if choices.len() > limit {
            result.has_more = true;
            choices.select_nth_unstable_by(limit, compare);
            choices.truncate(limit);
        }
        choices.sort_unstable_by(compare);
        let mut materialize = self.connection.prepare(
            "SELECT d.node_id,d.file_id,s.path,d.name,d.start_line,d.end_line,d.start_byte,d.end_byte,
                    s.checksum,d.checksum,s.bytes
             FROM document d JOIN source s ON s.id=d.file_id WHERE d.id=?1",
        )?;
        for (id, document) in choices {
            result.hits.push(materialize.query_row([id], |row| {
                Ok(Hit {
                    node_id: NodeId(row.get(0)?),
                    file_id: row.get(1)?,
                    path: row.get(2)?,
                    name: row.get(3)?,
                    start_line: row.get(4)?,
                    end_line: row.get(5)?,
                    start_byte: row.get(6)?,
                    end_byte: row.get(7)?,
                    file_sha256: row.get(8)?,
                    body_sha256: row.get(9)?,
                    file_bytes: read_size(row, 10)? as u64,
                    score: document.score,
                })
            })?);
        }
        Ok(result)
    }
}

fn code_terms(input: &str) -> impl Iterator<Item = String> + '_ {
    input
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            let mut pieces = vec![word.to_ascii_lowercase()];
            for segment in word.split('_').filter(|segment| !segment.is_empty()) {
                let bytes = segment.as_bytes();
                let mut start = 0;
                for i in 1..bytes.len() {
                    let prev = bytes[i - 1];
                    let cur = bytes[i];
                    let boundary = cur.is_ascii_digit() != prev.is_ascii_digit()
                        || (cur.is_ascii_uppercase() && prev.is_ascii_lowercase())
                        || (cur.is_ascii_uppercase()
                            && prev.is_ascii_uppercase()
                            && bytes.get(i + 1).is_some_and(u8::is_ascii_lowercase));
                    if boundary {
                        if !segment[start..i].eq_ignore_ascii_case(word) {
                            pieces.push(segment[start..i].to_ascii_lowercase());
                        }
                        start = i;
                    }
                }
                if !segment[start..].eq_ignore_ascii_case(word) {
                    pieces.push(segment[start..].to_ascii_lowercase());
                }
            }
            pieces
        })
        .filter(|term| {
            !matches!(
                term.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "of"
                    | "this"
                    | "its"
                    | "is"
                    | "are"
                    | "for"
                    | "by"
                    | "as"
                    | "and"
                    | "or"
                    | "in"
                    | "that"
                    | "which"
                    | "to"
                    | "be"
                    | "it"
                    | "on"
                    | "from"
                    | "with"
                    | "at"
                    | "has"
                    | "if"
                    | "into"
                    | "when"
                    | "will"
                    | "there"
                    | "their"
                    | "they"
                    | "may"
                    | "can"
                    | "does"
                    | "function"
                    | "input"
                    | "output"
                    | "purpose"
                    | "algorithm"
                    | "return"
                    | "returns"
                    | "accepts"
                    | "takes"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_index_ranks_body_words_and_ties_within_corpus_and_filters_before_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CALLABLE_INDEX_FILE);
        write(
            &path,
            "old-gen",
            "run",
            None,
            |w| {
                for (id, file, body) in [
                    (1, "z.go", "amberPacket amberPacket burst"),
                    (2, "b.go", "amberPacket"),
                    (3, "a.go", "amberPacket"),
                ] {
                    w.file(id, file, "filehash", body.len())?;
                    w.file_status(id, id == 1, false, 0)?;
                    w.definition(
                        Definition {
                            node_id: NodeId(id),
                            file_id: id,
                            name: "apply",
                            start_line: 1,
                            end_line: 1,
                            start_byte: 0,
                            end_byte: body.len() as u32,
                            body_sha256: "bodyhash",
                        },
                        file,
                        body,
                    )?;
                }
                Ok(())
            },
            || false,
        )
        .unwrap();
        assert!(open(&path, "old-gen", "other").is_err());
        let reader = open(&path, "old-gen", "run").unwrap();
        assert_eq!(reader.coverage.indexed_callables, 3);
        assert_eq!(reader.coverage.partial_files, 1);
        let rank = reader
            .rank(
                "How is the amber packet burst handled?",
                3,
                |_| true,
                &|| false,
            )
            .unwrap();
        assert_eq!(
            rank.hits
                .iter()
                .map(|r| r.path.as_str())
                .collect::<Vec<_>>(),
            ["z.go", "a.go", "b.go"]
        );
        let scoped = reader
            .rank("amber packet burst", 1, |p| p != "z.go", &|| false)
            .unwrap();
        assert_eq!(scoped.hits[0].path, "a.go");
        assert!(scoped.has_more && !scoped.truncated);
        assert!(
            reader
                .rank("amber packet burst", 5, |_| true, &|| true)
                .is_err()
        );
    }

    #[test]
    fn callable_index_revises_only_changed_file_and_keeps_old_generation() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.sqlite3");
        let new = dir.path().join("new.sqlite3");
        let add = |w: &mut Writer<'_>, id, path, text: &str, partial| -> Result<()> {
            w.file_status(id, partial, false, 0)?;
            w.file(id, path, "filehash", text.len())?;
            w.definition(
                Definition {
                    node_id: NodeId(id),
                    file_id: id,
                    name: "apply",
                    start_line: 1,
                    end_line: 1,
                    start_byte: 0,
                    end_byte: text.len() as u32,
                    body_sha256: "bodyhash",
                },
                path,
                text,
            )
        };
        write(
            &old,
            "old",
            "oldrun",
            None,
            |w| {
                add(w, 1, "changed.go", "amberPacket duplicateTicket", true)?;
                add(w, 2, "retained.go", "duplicateTicket", false)
            },
            || false,
        )
        .unwrap();
        write(
            &new,
            "new",
            "newrun",
            Some(PreviousGeneration {
                path: &old,
                generation: "old",
                run: "oldrun",
                changed_file_ids: &[1],
            }),
            |w| add(w, 1, "changed.go", "blackPacket", false),
            || false,
        )
        .unwrap();
        let before = open(&old, "old", "oldrun").unwrap();
        let after = open(&new, "new", "newrun").unwrap();
        assert_eq!(before.coverage.partial_files, 1);
        assert_eq!(after.coverage.partial_files, 0);
        assert_eq!(after.coverage.indexed_callables, 2);
        assert!(
            after
                .rank("amber", 2, |_| true, &|| false)
                .unwrap()
                .hits
                .is_empty()
        );
        assert_eq!(
            after
                .rank("duplicateTicket", 2, |_| true, &|| false)
                .unwrap()
                .hits[0]
                .node_id,
            NodeId(2)
        );
        assert_eq!(
            after
                .rank("blackPacket", 2, |_| true, &|| false)
                .unwrap()
                .hits[0]
                .node_id,
            NodeId(1)
        );
        assert_eq!(
            before.rank("amber", 2, |_| true, &|| false).unwrap().hits[0].node_id,
            NodeId(1)
        );
    }
}
