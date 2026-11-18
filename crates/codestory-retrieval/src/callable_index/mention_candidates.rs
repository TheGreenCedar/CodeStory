//! Candidate occurrences from bounded posting lists. Callers verify exact spelling.

use super::*;
use std::collections::{BTreeSet, HashSet};

const MAX_SEED_POSTINGS: usize = 2_048;
const MAX_CONTEXT_TERMS: usize = 16;

impl Reader {
    /// Retrieve declarations whose indexed text could mention an anchor.
    /// An anchor is its member name and, for a common name, an optional type
    /// qualifier. Exclude original candidates before ranking the posting list.
    pub fn rank_reference_candidates(
        &self,
        anchors: &[(&str, Option<&str>)],
        question: &str,
        excluded: &[NodeId],
        limit: usize,
        in_scope: impl Fn(&str) -> bool,
    ) -> Result<RankedResult> {
        let mut result = RankedResult::default();
        let limit = limit.min(36);
        if limit == 0 {
            return Ok(result);
        }
        let excluded = excluded.iter().copied().collect::<HashSet<_>>();
        let anchor_terms: BTreeSet<_> = anchors
            .iter()
            .flat_map(|(member, qualifier)| std::iter::once(*member).chain(*qualifier))
            .flat_map(code_terms)
            .collect();
        let mut lookup = self
            .connection
            .prepare("SELECT id,documents FROM term WHERE name=?1")?;
        let mut lookup_term = |term: &str| -> Result<Option<(usize, i64)>> {
            Ok(lookup
                .query_row([term], |row| Ok((read_size(row, 1)?, row.get(0)?)))
                .optional()?)
        };
        let question_end = question.floor_char_boundary(MAX_QUERY_BYTES.min(question.len()));
        let context = code_terms(&question[..question_end])
            .filter(|word| !anchor_terms.contains(word))
            .take(MAX_QUERY_TERMS)
            .collect::<BTreeSet<_>>();
        let mut context_terms = Vec::new();
        for word in context {
            context_terms.extend(lookup_term(&word)?);
        }
        context_terms.sort_unstable();
        context_terms.truncate(MAX_CONTEXT_TERMS);

        let mut seed = self.connection.prepare(
            "SELECT p.doc_id,d.node_id,s.path,d.start_line,d.end_line
             FROM posting p JOIN document d ON d.id=p.doc_id JOIN source s ON s.id=d.file_id
             WHERE p.term_id=?1 ORDER BY p.doc_id",
        )?;
        let mut contains = self
            .connection
            .prepare("SELECT 1 FROM posting WHERE term_id=?1 AND doc_id=?2")?;
        let mut scores = HashMap::<i64, RankedDocument>::new();
        for (member, qualifier) in anchors.iter().take(2) {
            let Some(member) = lookup_term(&member.to_ascii_lowercase())? else {
                continue;
            };
            let qualifier = match qualifier {
                Some(word) => {
                    let Some(found) = lookup_term(&word.to_ascii_lowercase())? else {
                        continue;
                    };
                    Some(found)
                }
                None => None,
            };
            let (primary, secondary) = match qualifier {
                Some(word) if word < member => (word, Some(member)),
                word => (member, word),
            };
            if primary.0 > MAX_SEED_POSTINGS {
                result.truncated = true;
                continue;
            }
            let mut rows = seed.query([primary.1])?;
            let mut seen = 0;
            while let Some(row) = rows.next()? {
                seen += 1;
                if seen > primary.0 {
                    bail!("reference posting count does not match the term metadata");
                }
                let document: i64 = row.get(0)?;
                let node = NodeId(row.get(1)?);
                let path = row.get_ref(2)?.as_str()?;
                if excluded.contains(&node) || !in_scope(path) || scores.contains_key(&document) {
                    continue;
                }
                if let Some((_, term)) = secondary
                    && !contains.exists(params![term, document])?
                {
                    continue;
                }
                let mut score = 0.0;
                for (frequency, term) in &context_terms {
                    if contains.exists(params![term, document])? {
                        score += (1.0 + 1_000.0 / (*frequency as f64 + 1.0)).ln();
                    }
                }
                scores.insert(
                    document,
                    RankedDocument {
                        score,
                        path: Arc::from(path),
                        start: row.get(3)?,
                        end: row.get(4)?,
                    },
                );
            }
            if seen != primary.0 {
                bail!("reference posting count does not match the term metadata");
            }
        }
        let mut ranked = scores.into_iter().collect::<Vec<_>>();
        ranked.sort_unstable_by(|(id, a), (other, b)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| (&a.path, a.start, a.end, id).cmp(&(&b.path, b.start, b.end, other)))
        });
        result.has_more = ranked.len() > limit;
        let mut materialize = self.connection.prepare(
            "SELECT d.node_id,d.file_id,s.path,d.name,d.start_line,d.end_line,d.start_byte,d.end_byte,
                    s.checksum,d.checksum,s.bytes
             FROM document d JOIN source s ON s.id=d.file_id WHERE d.id=?1",
        )?;
        for (id, document) in ranked.into_iter().take(limit) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_postings_require_receiver_scope_and_exclude_the_initial_definition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CALLABLE_INDEX_FILE);
        write(
            &path,
            "generation",
            "run",
            None,
            |writer| {
                for (id, file, body) in [
                    (1, "src/parser.go", "Parser.Read retry payload schedule"),
                    (2, "src/false.go", "Other.Read retry payload schedule"),
                    (3, "src/receiver.go", "Parser.Read retry payload"),
                    (4, "tests/fixture.go", "Parser.Read retry payload schedule"),
                    (5, "src/weak.go", "Parser.Read"),
                ] {
                    writer.file(id, file, "filehash", body.len())?;
                    writer.file_status(id, false, false, 0)?;
                    writer.definition(
                        Definition {
                            node_id: NodeId(id),
                            file_id: id,
                            name: "handler",
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
        let reader = open(&path, "generation", "run").unwrap();
        let reply = reader
            .rank_reference_candidates(
                &[("Read", Some("Parser"))],
                "Parser.Read retry payload schedule",
                &[NodeId(1)],
                1,
                |path| path.starts_with("src/"),
            )
            .unwrap();
        assert_eq!(
            reply.hits.iter().map(|hit| hit.node_id).collect::<Vec<_>>(),
            [NodeId(3)]
        );
        assert!(reply.has_more);
    }
}
