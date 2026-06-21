//! Catalog for the stdio integration surface.
//!
//! This module defines the tool/resource/prompt metadata exposed by
//! `serve --stdio`. The catalog is declarative JSON shape: keep names, schemas,
//! annotations, and safety metadata stable because clients discover behavior
//! from these responses before calling into the transport.

use anyhow::Result;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Side-effect class advertised for a stdio tool.
///
/// The current stdio surface is read-only and local-only; adding a new effect
/// requires updating both safety metadata and tool annotations.
pub(crate) enum ToolEffect {
    Read,
}

#[derive(Debug, Clone, Copy)]
/// Safety metadata emitted in both legacy and annotation-style catalog fields.
pub(crate) struct SafetyMetadata {
    effect: ToolEffect,
}

impl SafetyMetadata {
    pub(crate) const fn read_only() -> Self {
        Self {
            effect: ToolEffect::Read,
        }
    }

    fn to_json(self) -> Value {
        match self.effect {
            ToolEffect::Read => json!({
                "readOnly": true,
                "sideEffects": false,
                "localOnly": true,
                "openWorld": false
            }),
        }
    }

    fn annotations_json(self) -> Value {
        match self.effect {
            ToolEffect::Read => json!({
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Declarative stdio tool description returned by `tools/list`.
pub(crate) struct ToolSpec {
    name: &'static str,
    description: &'static str,
    input_schema: SchemaObject,
    output_schema: Option<SchemaSpec>,
    safety: SafetyMetadata,
}

impl ToolSpec {
    fn to_json(self) -> Value {
        let mut tool = Map::from_iter([
            ("name".to_string(), json!(self.name)),
            ("description".to_string(), json!(self.description)),
            ("inputSchema".to_string(), self.input_schema.to_json()),
            ("safety".to_string(), self.safety.to_json()),
            ("annotations".to_string(), self.safety.annotations_json()),
        ]);
        if let Some(output_schema) = self.output_schema {
            tool.insert("outputSchema".to_string(), output_schema.to_json());
        }
        Value::Object(tool)
    }
}

#[derive(Debug, Clone, Copy)]
/// Declarative resource description returned by `resources/list`.
pub(crate) struct ResourceSpec {
    uri: &'static str,
    name: &'static str,
    mime_type: &'static str,
}

impl ResourceSpec {
    fn to_json(self) -> Value {
        json!({
            "uri": self.uri,
            "name": self.name,
            "mimeType": self.mime_type
        })
    }
}

#[derive(Debug, Clone, Copy)]
/// Declarative resource template returned by `resources/templates/list`.
pub(crate) struct ResourceTemplateSpec {
    uri_template: &'static str,
    name: &'static str,
    mime_type: &'static str,
}

impl ResourceTemplateSpec {
    fn to_json(self) -> Value {
        json!({
            "uriTemplate": self.uri_template,
            "name": self.name,
            "mimeType": self.mime_type
        })
    }
}

#[derive(Debug, Clone, Copy)]
/// Declarative prompt description returned by `prompts/list` and `prompts/get`.
pub(crate) struct PromptSpec {
    name: &'static str,
    description: &'static str,
    text: &'static str,
}

impl PromptSpec {
    fn list_json(self) -> Value {
        json!({
            "name": self.name,
            "description": self.description
        })
    }

    fn get_json(self) -> Value {
        json!({
            "result": {
                "description": self.description,
                "messages": [
                    {
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": self.text
                        }
                    }
                ]
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum SchemaType {
    Object,
    Array,
    String,
    Integer,
    Boolean,
    Number,
}

impl SchemaType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Array => "array",
            Self::String => "string",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Number => "number",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SchemaSpec {
    Object(SchemaObject),
}

impl SchemaSpec {
    fn to_json(self) -> Value {
        match self {
            Self::Object(object) => object.to_json(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SchemaItems {
    Object(&'static SchemaObject),
    Type(SchemaType),
}

impl SchemaItems {
    fn to_json(self) -> Value {
        match self {
            Self::Object(object) => object.to_json(),
            Self::Type(schema_type) => json!({ "type": schema_type.as_str() }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SchemaProperty {
    name: &'static str,
    schema_type: SchemaType,
    description: Option<&'static str>,
    enum_values: &'static [&'static str],
    default: Option<ValueLiteral>,
    minimum: Option<u64>,
    maximum: Option<u64>,
    min_length: Option<u64>,
    items: Option<SchemaItems>,
    nullable: bool,
}

impl SchemaProperty {
    const fn string(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::String,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: None,
            nullable: false,
        }
    }

    const fn string_required(name: &'static str, description: &'static str) -> Self {
        Self::string(name, description)
    }

    const fn integer(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::Integer,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: None,
            nullable: false,
        }
    }

    const fn boolean(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::Boolean,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: None,
            nullable: false,
        }
    }

    const fn number(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::Number,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: None,
            nullable: false,
        }
    }

    const fn object(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::Object,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: None,
            nullable: false,
        }
    }

    const fn array(
        name: &'static str,
        description: &'static str,
        items: &'static SchemaObject,
    ) -> Self {
        Self {
            name,
            schema_type: SchemaType::Array,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: Some(SchemaItems::Object(items)),
            nullable: false,
        }
    }

    const fn string_array(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            schema_type: SchemaType::Array,
            description: Some(description),
            enum_values: &[],
            default: None,
            minimum: None,
            maximum: None,
            min_length: None,
            items: Some(SchemaItems::Type(SchemaType::String)),
            nullable: false,
        }
    }

    const fn with_enum(mut self, enum_values: &'static [&'static str]) -> Self {
        self.enum_values = enum_values;
        self
    }

    const fn with_default(mut self, default: ValueLiteral) -> Self {
        self.default = Some(default);
        self
    }

    const fn with_bounds(mut self, minimum: u64, maximum: u64) -> Self {
        self.minimum = Some(minimum);
        self.maximum = Some(maximum);
        self
    }

    const fn with_min_length(mut self, min_length: u64) -> Self {
        self.min_length = Some(min_length);
        self
    }

    const fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    fn to_json(self) -> Value {
        let type_json = if self.nullable {
            json!([self.schema_type.as_str(), "null"])
        } else {
            json!(self.schema_type.as_str())
        };
        let mut schema = Map::from_iter([("type".to_string(), type_json)]);
        if let Some(description) = self.description {
            schema.insert("description".to_string(), json!(description));
        }
        if !self.enum_values.is_empty() {
            schema.insert("enum".to_string(), json!(self.enum_values));
        }
        if let Some(default) = self.default {
            schema.insert("default".to_string(), default.to_json());
        }
        if let Some(minimum) = self.minimum {
            schema.insert("minimum".to_string(), json!(minimum));
        }
        if let Some(maximum) = self.maximum {
            schema.insert("maximum".to_string(), json!(maximum));
        }
        if let Some(min_length) = self.min_length {
            schema.insert("minLength".to_string(), json!(min_length));
        }
        if let Some(items) = self.items {
            schema.insert("items".to_string(), items.to_json());
        }
        Value::Object(schema)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ValueLiteral {
    String(&'static str),
    Integer(u64),
    Boolean(bool),
}

impl ValueLiteral {
    fn to_json(self) -> Value {
        match self {
            Self::String(value) => json!(value),
            Self::Integer(value) => json!(value),
            Self::Boolean(value) => json!(value),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SchemaObject {
    description: Option<&'static str>,
    properties: &'static [SchemaProperty],
    required: &'static [&'static str],
    any_of_required: &'static [&'static [&'static str]],
    additional_properties: bool,
}

impl SchemaObject {
    const fn object(
        description: &'static str,
        properties: &'static [SchemaProperty],
        required: &'static [&'static str],
    ) -> Self {
        Self {
            description: Some(description),
            properties,
            required,
            any_of_required: &[],
            additional_properties: false,
        }
    }

    const fn passthrough_object(description: &'static str) -> Self {
        Self {
            description: Some(description),
            properties: &[],
            required: &[],
            any_of_required: &[],
            additional_properties: true,
        }
    }

    const fn with_any_of_required(
        mut self,
        any_of_required: &'static [&'static [&'static str]],
    ) -> Self {
        self.any_of_required = any_of_required;
        self
    }

    fn to_json(self) -> Value {
        let properties = self
            .properties
            .iter()
            .map(|property| (property.name.to_string(), property.to_json()))
            .collect::<Map<_, _>>();
        let mut schema = Map::from_iter([
            ("type".to_string(), json!("object")),
            ("properties".to_string(), Value::Object(properties)),
            (
                "additionalProperties".to_string(),
                json!(self.additional_properties),
            ),
        ]);
        if let Some(description) = self.description {
            schema.insert("description".to_string(), json!(description));
        }
        schema.insert("required".to_string(), json!(self.required));
        if !self.any_of_required.is_empty() {
            schema.insert(
                "anyOf".to_string(),
                Value::Array(
                    self.any_of_required
                        .iter()
                        .map(|required| json!({ "required": required }))
                        .collect(),
                ),
            );
        }
        Value::Object(schema)
    }
}

const TEXT_HIT_ORIGINS: &[&str] = &["indexed_symbol", "text_match"];
const SEARCH_REPO_TEXT_MODES: &[&str] = &["auto", "on", "off"];
const INDEXED_FILE_ROLES: &[&str] = &["source", "test", "generated", "vendor", "unknown"];
const SNIPPET_SCOPES: &[&str] = &["line_context", "function_body"];
const GROUNDING_BUDGETS: &[&str] = &["strict", "balanced", "max"];
const PACKET_BUDGETS: &[&str] = &["tiny", "compact", "standard", "deep"];
const PACKET_TASK_CLASSES: &[&str] = &[
    "architecture_explanation",
    "bug_localization",
    "change_impact",
    "route_tracing",
    "symbol_ownership",
    "data_flow",
    "edit_planning",
];

static GENERIC_OBJECT_SCHEMA: SchemaObject =
    SchemaObject::passthrough_object("Generic JSON object.");

static RESOURCE_LINK_SCHEMA: SchemaObject = SchemaObject::object(
    "Continuation resource link.",
    &[
        SchemaProperty::string("rel", "Link relation."),
        SchemaProperty::string("uri", "CodeStory resource URI."),
    ],
    &["rel", "uri"],
);

static SEARCH_HIT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory search hit DTO.",
    &[
        SchemaProperty::string_required("node_id", "Stable node id.").with_min_length(1),
        SchemaProperty::string("display_name", "Display name."),
        SchemaProperty::string("kind", "Node kind."),
        SchemaProperty::string("file_path", "Project-relative file path.").nullable(),
        SchemaProperty::integer("line", "One-based line number.").nullable(),
        SchemaProperty::number("score", "Ranking score."),
        SchemaProperty::string("origin", "Hit source.").with_enum(TEXT_HIT_ORIGINS),
        SchemaProperty::string(
            "match_quality",
            "How exactly the hit matched the query: exact, normalized_exact, prefix, fuzzy, semantic_suggestion, or repo_text.",
        ),
        SchemaProperty::boolean(
            "resolvable",
            "Whether the hit can be used as a symbol target.",
        ),
        SchemaProperty::object("score_breakdown", "Optional retrieval score breakdown."),
        SchemaProperty::array(
            "links",
            "Bounded continuation resource links for this hit.",
            &RESOURCE_LINK_SCHEMA,
        ),
    ],
    &[
        "node_id",
        "display_name",
        "kind",
        "score",
        "origin",
        "resolvable",
    ],
);

static SYMBOL_SUMMARY_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory symbol summary DTO.",
    &[
        SchemaProperty::string_required("id", "Stable node id.").with_min_length(1),
        SchemaProperty::string("label", "Symbol label."),
        SchemaProperty::string("kind", "Node kind."),
        SchemaProperty::string("file_path", "Project-relative file path.").nullable(),
        SchemaProperty::boolean("has_children", "Whether children can be browsed."),
    ],
    &["id", "label", "kind", "has_children"],
);

static SEARCH_RESULTS_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory discovery results DTO. Treat broad structural questions as packet-first; search rows select candidates for proof-bearing graph/source follow-up.",
    &[
        SchemaProperty::string("query", "Search query."),
        SchemaProperty::object("retrieval", "Retrieval state DTO."),
        SchemaProperty::object(
            "retrieval_shadow",
            "Optional sidecar shadow retrieval trace DTO.",
        )
        .nullable(),
        SchemaProperty::integer("limit_per_source", "Per-source result limit."),
        SchemaProperty::string("repo_text_mode", "Repo text search mode.")
            .with_enum(SEARCH_REPO_TEXT_MODES),
        SchemaProperty::boolean("repo_text_enabled", "Whether repo text search was enabled."),
        SchemaProperty::object(
            "query_assessment",
            "Exactness, weak-hit, repo-text diagnostic, and next-action assessment.",
        )
        .nullable(),
        SchemaProperty::object(
            "search_plan",
            "Optional broad natural-language Search Plan with subqueries, anchor groups, bridge evidence, next commands, and source-truth checks.",
        )
        .nullable(),
        SchemaProperty::object(
            "repo_text_stats",
            "Repo text scan cap, byte, and truncation telemetry.",
        )
        .nullable(),
        SchemaProperty::array(
            "suggestions",
            "Alternative matching symbols.",
            &SEARCH_HIT_SCHEMA,
        ),
        SchemaProperty::array(
            "indexed_symbol_hits",
            "Indexed symbol hits.",
            &SEARCH_HIT_SCHEMA,
        ),
        SchemaProperty::array("repo_text_hits", "Repo text hits.", &SEARCH_HIT_SCHEMA),
        SchemaProperty::array("hits", "Merged hit list.", &SEARCH_HIT_SCHEMA),
        SchemaProperty::string("code", "Typed API error code."),
        SchemaProperty::string("message", "Human-readable API error message."),
        SchemaProperty::object("details", "Structured API error repair guidance.").nullable(),
    ],
    &[],
)
.with_any_of_required(&[
    &[
        "query",
        "retrieval",
        "limit_per_source",
        "repo_text_mode",
        "repo_text_enabled",
        "hits",
    ],
    &["code", "message"],
]);

static SYMBOL_CONTEXT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory symbol context DTO.",
    &[
        SchemaProperty::object("node", "Node details DTO."),
        SchemaProperty::string("summary", "Optional generated summary.").nullable(),
        SchemaProperty::array(
            "children",
            "Child symbol summaries.",
            &SYMBOL_SUMMARY_SCHEMA,
        ),
        SchemaProperty::array("related_hits", "Related search hits.", &SEARCH_HIT_SCHEMA),
        SchemaProperty::string_array("edge_digest", "Readable edge digest entries."),
    ],
    &["node", "children", "related_hits", "edge_digest"],
);

static SYMBOLS_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory symbol list output.",
    &[SchemaProperty::array(
        "symbols",
        "Root or child symbol summaries.",
        &SYMBOL_SUMMARY_SCHEMA,
    )],
    &["symbols"],
);

static INDEXED_FILE_SCHEMA: SchemaObject = SchemaObject::object(
    "Indexed file coverage row.",
    &[
        SchemaProperty::string("path", "Project-relative file path."),
        SchemaProperty::string("language", "Detected language."),
        SchemaProperty::boolean("indexed", "Whether the file was indexed."),
        SchemaProperty::boolean("complete", "Whether indexing completed for this file."),
        SchemaProperty::integer("line_count", "Line count."),
        SchemaProperty::string("role", "Inferred file role.").with_enum(INDEXED_FILE_ROLES),
        SchemaProperty::integer("error_count", "File-level index error count."),
    ],
    &[
        "path",
        "language",
        "indexed",
        "complete",
        "line_count",
        "role",
    ],
);

static INDEXED_FILES_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Indexed file inventory and coverage summary.",
    &[
        SchemaProperty::string("project_root", "Project root."),
        SchemaProperty::boolean("usable", "Whether the index has usable files."),
        SchemaProperty::object("summary", "Indexed file summary DTO."),
        SchemaProperty::array("files", "Indexed file rows.", &INDEXED_FILE_SCHEMA),
        SchemaProperty::string("code", "Typed API error code."),
        SchemaProperty::string("message", "Human-readable API error message."),
        SchemaProperty::object("details", "Structured API error repair guidance.").nullable(),
    ],
    &[],
)
.with_any_of_required(&[
    &["project_root", "usable", "summary", "files"],
    &["code", "message"],
]);

static GROUNDING_SNAPSHOT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory grounding snapshot DTO for compact repository orientation.",
    &[
        SchemaProperty::string("root", "Project root."),
        SchemaProperty::string("budget", "Grounding output budget.").with_enum(GROUNDING_BUDGETS),
        SchemaProperty::integer("generated_at_epoch_ms", "Snapshot generation time."),
        SchemaProperty::object("stats", "Indexed project stats."),
        SchemaProperty::object("retrieval", "Optional retrieval state DTO.").nullable(),
        SchemaProperty::object("coverage", "Grounding coverage summary."),
        SchemaProperty::array(
            "root_symbols",
            "Root symbol digests.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::array("files", "File digests.", &GENERIC_OBJECT_SCHEMA),
        SchemaProperty::array(
            "coverage_buckets",
            "Compressed coverage buckets.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::string_array("notes", "Grounding notes."),
        SchemaProperty::string_array("recommended_queries", "Suggested follow-up queries."),
    ],
    &[
        "root",
        "budget",
        "generated_at_epoch_ms",
        "stats",
        "coverage",
        "root_symbols",
        "files",
    ],
);

static TRAIL_CONTEXT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory trail context DTO.",
    &[
        SchemaProperty::object("focus", "Focused node details DTO."),
        SchemaProperty::object("trail", "Graph response DTO."),
        SchemaProperty::object("story", "Optional readable trail story DTO.").nullable(),
    ],
    &["focus", "trail"],
);

static GRAPH_TOOL_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Bounded CodeStory graph primitive output.",
    &[
        SchemaProperty::object("resolution", "Optional query resolution metadata.").nullable(),
        SchemaProperty::object("node", "Node details DTO.").nullable(),
        SchemaProperty::object("graph", "Graph response DTO.").nullable(),
        SchemaProperty::string("certainty", "Overall certainty note."),
        SchemaProperty::array(
            "file_refs",
            "Stable project-relative file references.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::object("limits", "Applied bounds for this graph primitive."),
        SchemaProperty::integer("node_count", "Returned node count."),
        SchemaProperty::integer("edge_count", "Returned edge count."),
        SchemaProperty::boolean("truncated", "Whether the graph result was truncated."),
    ],
    &[
        "certainty",
        "file_refs",
        "limits",
        "node_count",
        "edge_count",
        "truncated",
    ],
);

static SNIPPET_CONTEXT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory snippet context DTO.",
    &[
        SchemaProperty::object("node", "Node details DTO."),
        SchemaProperty::string("path", "Project-relative file path."),
        SchemaProperty::integer("line", "One-based focused line."),
        SchemaProperty::string("snippet", "Source snippet text."),
        SchemaProperty::string("scope", "Snippet scope.").with_enum(SNIPPET_SCOPES),
        SchemaProperty::integer("requested_context", "Requested context line count."),
        SchemaProperty::boolean("snippet_truncated", "Whether the snippet hit a byte cap."),
        SchemaProperty::integer("max_snippet_bytes", "Snippet byte cap.").nullable(),
    ],
    &[
        "node",
        "path",
        "line",
        "snippet",
        "scope",
        "requested_context",
        "snippet_truncated",
    ],
);

static DEFINITION_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory definition tool output.",
    &[
        SchemaProperty::object("resolution", "Query resolution metadata."),
        SchemaProperty::object("definition", "Resolved definition search hit."),
        SchemaProperty::object("symbol", "Symbol context DTO."),
        SchemaProperty::array(
            "links",
            "Continuation resource links for the resolved definition.",
            &RESOURCE_LINK_SCHEMA,
        ),
    ],
    &["resolution", "definition", "symbol"],
);

static AGENT_CITATION_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory agent citation DTO.",
    &[
        SchemaProperty::string_required("node_id", "Stable node id.").with_min_length(1),
        SchemaProperty::string("display_name", "Display name."),
        SchemaProperty::string("kind", "Node kind."),
        SchemaProperty::string("file_path", "Project-relative file path.").nullable(),
        SchemaProperty::integer("line", "One-based line number.").nullable(),
        SchemaProperty::number("score", "Citation score."),
        SchemaProperty::string(
            "origin",
            "Citation origin, such as indexed_symbol or text_match.",
        )
        .with_enum(TEXT_HIT_ORIGINS),
        SchemaProperty::boolean(
            "resolvable",
            "Whether the citation can be resolved as a symbol.",
        ),
        SchemaProperty::string("subgraph_id", "Related subgraph id.").nullable(),
        SchemaProperty::string_array("evidence_edge_ids", "Evidence edge ids."),
        SchemaProperty::object(
            "retrieval_score_breakdown",
            "Optional retrieval score breakdown.",
        )
        .nullable(),
    ],
    &[
        "node_id",
        "display_name",
        "kind",
        "score",
        "origin",
        "resolvable",
    ],
);

static CONTEXT_PACKET_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory context packet DTO.",
    &[
        SchemaProperty::string("packet_id", "Stable context packet id."),
        SchemaProperty::string("target", "Resolved retrieval target label."),
        SchemaProperty::string("summary", "Context packet summary."),
        SchemaProperty::array("sections", "Context sections.", &GENERIC_OBJECT_SCHEMA),
        SchemaProperty::array("citations", "Evidence citations.", &AGENT_CITATION_SCHEMA),
        SchemaProperty::string_array("subgraph_ids", "Related graph ids."),
        SchemaProperty::string("retrieval_version", "Retrieval version."),
        SchemaProperty::array("graphs", "Graph artifacts.", &GENERIC_OBJECT_SCHEMA),
        SchemaProperty::object("retrieval_trace", "Retrieval trace and summary."),
    ],
    &[
        "packet_id",
        "target",
        "summary",
        "sections",
        "citations",
        "subgraph_ids",
        "retrieval_version",
        "graphs",
        "retrieval_trace",
    ],
);

static AGENT_PACKET_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory broad task packet DTO with graph/sidecar evidence, budget truncation, unsafe-to-claim gaps, and follow-up commands.",
    &[
        SchemaProperty::string("packet_id", "Stable packet id."),
        SchemaProperty::string("question", "Packet question."),
        SchemaProperty::string("task_class", "Optional task class.")
            .with_enum(PACKET_TASK_CLASSES)
            .nullable(),
        SchemaProperty::object("plan", "Packet planner trace."),
        SchemaProperty::object("answer", "Underlying DB-first answer packet."),
        SchemaProperty::object("budget", "Budget limits, usage, and truncation metadata."),
        SchemaProperty::object(
            "sufficiency",
            "Covered claims, gaps, and follow-up contract.",
        ),
        SchemaProperty::object(
            "retrieval_trace_summary",
            "Compact retrieval trace telemetry summary.",
        ),
    ],
    &[
        "packet_id",
        "question",
        "plan",
        "answer",
        "budget",
        "sufficiency",
        "retrieval_trace_summary",
    ],
);

static SEARCH_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Search indexed symbols and repo text.",
    &[
        SchemaProperty::string_required("query", "Search query.").with_min_length(1),
        SchemaProperty::string("repo_text", "Repo text search mode.")
            .with_enum(SEARCH_REPO_TEXT_MODES)
            .with_default(ValueLiteral::String("auto")),
        SchemaProperty::integer("limit", "Maximum hits returned.")
            .with_default(ValueLiteral::Integer(10))
            .with_bounds(1, 50),
    ],
    &["query"],
);

static GROUND_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return the same compact repository orientation as codestory://grounding.",
    &[SchemaProperty::string("budget", "Grounding output budget.")
        .with_enum(GROUNDING_BUDGETS)
        .with_default(ValueLiteral::String("balanced"))],
    &[],
);

static TRAIL_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a graph trail around a symbol id or query.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
        SchemaProperty::string("direction", "Trail direction.")
            .with_enum(&["incoming", "outgoing", "both"])
            .with_default(ValueLiteral::String("both")),
        SchemaProperty::integer("depth", "Trail depth.")
            .with_default(ValueLiteral::Integer(2))
            .with_bounds(0, 10),
        SchemaProperty::boolean("story", "Include a readable trail story DTO.")
            .with_default(ValueLiteral::Boolean(false)),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static TARGET_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Resolve a symbol by query or stable node id.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static GRAPH_TARGET_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Resolve a single indexed graph node by stable id or query.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static GRAPH_NEIGHBORS_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a bounded graph neighborhood around one node.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
        SchemaProperty::string("direction", "Graph direction.")
            .with_enum(&["incoming", "outgoing", "both"])
            .with_default(ValueLiteral::String("both")),
        SchemaProperty::integer("depth", "Graph depth.")
            .with_default(ValueLiteral::Integer(1))
            .with_bounds(0, 3),
        SchemaProperty::integer("max_nodes", "Maximum graph nodes returned.")
            .with_default(ValueLiteral::Integer(50))
            .with_bounds(1, 120),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static SHORTEST_PATH_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a bounded forward path graph between two stable node ids.",
    &[
        SchemaProperty::string("from_id", "Stable source node id.").with_min_length(1),
        SchemaProperty::string("to_id", "Stable target node id.").with_min_length(1),
        SchemaProperty::integer("max_depth", "Maximum path depth.")
            .with_default(ValueLiteral::Integer(6))
            .with_bounds(1, 10),
        SchemaProperty::integer("max_nodes", "Maximum graph nodes returned.")
            .with_default(ValueLiteral::Integer(80))
            .with_bounds(2, 120),
    ],
    &["from_id", "to_id"],
);

static QUERY_SUBGRAPH_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a bounded graph subgraph around one resolved node.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
        SchemaProperty::string("direction", "Graph direction.")
            .with_enum(&["incoming", "outgoing", "both"])
            .with_default(ValueLiteral::String("both")),
        SchemaProperty::integer("depth", "Graph depth.")
            .with_default(ValueLiteral::Integer(2))
            .with_bounds(0, 3),
        SchemaProperty::integer("max_nodes", "Maximum graph nodes returned.")
            .with_default(ValueLiteral::Integer(80))
            .with_bounds(1, 120),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static SYMBOLS_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Browse root symbols or children for a parent id.",
    &[
        SchemaProperty::string("parent_id", "Parent node id.").with_min_length(1),
        SchemaProperty::integer("limit", "Maximum root symbols returned.")
            .with_default(ValueLiteral::Integer(300))
            .with_bounds(1, 2000),
    ],
    &[],
);

static FILES_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "List indexed files from the existing local index.",
    &[
        SchemaProperty::string("path", "Only include files whose path contains this text."),
        SchemaProperty::string("language", "Only include files for this language."),
        SchemaProperty::string("role", "Only include files with this inferred role.")
            .with_enum(INDEXED_FILE_ROLES),
        SchemaProperty::integer("limit", "Maximum files returned.")
            .with_default(ValueLiteral::Integer(500))
            .with_bounds(1, 5000),
    ],
    &[],
);

static CONTEXT_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Build a deep evidence packet for one concrete retrieval target.",
    &[
        SchemaProperty::string(
            "query",
            "Concrete symbol, file, literal, API path, module, or behavior term.",
        )
        .with_min_length(1),
        SchemaProperty::string("id", "Stable node id to build context around.").with_min_length(1),
        SchemaProperty::string("bookmark", "Saved bookmark id to build context around.")
            .with_min_length(1),
        SchemaProperty::integer("max_results", "Maximum retrieval results.")
            .with_default(ValueLiteral::Integer(8))
            .with_bounds(1, 50),
        SchemaProperty::boolean(
            "include_evidence",
            "Include citation edge ids and score details.",
        )
        .with_default(ValueLiteral::Boolean(true)),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"], &["bookmark"]]);

static PACKET_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Build a broad task packet with budget and sufficiency metadata.",
    &[
        SchemaProperty::string_required("question", "Broad repository question or task.")
            .with_min_length(1),
        SchemaProperty::string("budget", "Packet budget.")
            .with_enum(PACKET_BUDGETS)
            .with_default(ValueLiteral::String("compact")),
        SchemaProperty::string("task_class", "Optional task class.")
            .with_enum(PACKET_TASK_CLASSES)
            .nullable(),
        SchemaProperty::string_array(
            "extra_probes",
            "Optional audited file, symbol, or file-scoped symbol probes to add to the packet plan.",
        ),
        SchemaProperty::boolean(
            "include_evidence",
            "Include citation edge ids and score details.",
        )
        .with_default(ValueLiteral::Boolean(true)),
        SchemaProperty::integer(
            "latency_budget_ms",
            "Optional retrieval latency budget in milliseconds.",
        )
        .with_bounds(1000, 120000)
        .nullable(),
    ],
    &["question"],
);

static TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "packet",
        description: "Answer broad structural questions with graph/sidecar evidence, sufficiency, truncation, and follow-up commands before source snippets.",
        input_schema: PACKET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(AGENT_PACKET_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "search",
        description: "Discover candidate symbols and sidecar hits; for broad structural questions call packet before snippet/source reads.",
        input_schema: SEARCH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SEARCH_RESULTS_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "ground",
        description: "Return a compact repository map for orientation after status and before packet/search; equivalent to codestory://grounding.",
        input_schema: GROUND_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GROUNDING_SNAPSHOT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "files",
        description: "List indexed files and coverage from the existing local index; never refreshes, indexes, or bootstraps sidecars.",
        input_schema: FILES_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(INDEXED_FILES_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "symbol",
        description: "Resolve a symbol id or query and return details.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SYMBOL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "trail",
        description: "Return a graph trail around a symbol.",
        input_schema: TRAIL_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(TRAIL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "get_node",
        description: "Return one stable graph node with file refs before requesting a packet.",
        input_schema: GRAPH_TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "neighbors",
        description: "Return a bounded graph neighborhood around one node.",
        input_schema: GRAPH_NEIGHBORS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "shortest_path",
        description: "Return a bounded forward path graph between two node ids.",
        input_schema: SHORTEST_PATH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "query_subgraph",
        description: "Return a bounded subgraph around one resolved node; packet remains the broad task tool.",
        input_schema: QUERY_SUBGRAPH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "definition",
        description: "Return definition metadata for a symbol id or query.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(DEFINITION_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "references",
        description: "Return incoming references for a symbol id or query.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(TRAIL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "symbols",
        description: "Browse root symbols or children for a parent id.",
        input_schema: SYMBOLS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SYMBOLS_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "snippet",
        description: "Return a focused source snippet after packet, search, or graph evidence selects a concrete target.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SNIPPET_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
    ToolSpec {
        name: "context",
        description: "Build proof-bearing source/graph evidence for one concrete target; not broad question answering.",
        input_schema: CONTEXT_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(CONTEXT_PACKET_SCHEMA)),
        safety: SafetyMetadata::read_only(),
    },
];

static RESOURCES: &[ResourceSpec] = &[
    ResourceSpec {
        uri: "codestory://status",
        name: "Status",
        mime_type: "application/json",
    },
    ResourceSpec {
        uri: "codestory://agent-guide",
        name: "Agent guide",
        mime_type: "application/json",
    },
    ResourceSpec {
        uri: "codestory://project",
        name: "Project summary",
        mime_type: "application/json",
    },
    ResourceSpec {
        uri: "codestory://grounding",
        name: "Grounding snapshot",
        mime_type: "application/json",
    },
    ResourceSpec {
        uri: "codestory://symbols/root",
        name: "Root symbols",
        mime_type: "application/json",
    },
];

static RESOURCE_TEMPLATES: &[ResourceTemplateSpec] = &[
    ResourceTemplateSpec {
        uri_template: "codestory://symbol/{node_id}",
        name: "Symbol details",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://references/{node_id}",
        name: "Symbol references",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://snippet/{node_id}",
        name: "Symbol snippet",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://trail/{node_id}",
        name: "Symbol trail",
        mime_type: "application/json",
    },
];

static PROMPTS: &[PromptSpec] = &[
    PromptSpec {
        name: "explain_symbol",
        description: "Explain a symbol using definition, references, and snippet context.",
        text: "Explain `{symbol}` using CodeStory definition, references, and source snippet context. Keep claims tied to retrieved evidence.",
    },
    PromptSpec {
        name: "trace_callflow",
        description: "Trace the outgoing call flow for a symbol.",
        text: "Trace the call flow for `{symbol}`. Use CodeStory trail, definition, and snippet context. Return key calls, uncertain edges, and review notes.",
    },
    PromptSpec {
        name: "impact_analysis",
        description: "Find incoming references and likely downstream impact.",
        text: "Analyze the impact of changing `{symbol}`. Use incoming references, related symbols, and snippets. Separate direct callers from broader risk.",
    },
];

/// Return whether a name is a registered stdio tool.
pub(crate) fn is_tool_name(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}

/// Build the `tools/list` response.
pub(crate) fn tools_list_json() -> Value {
    debug_assert_read_only_catalog();
    json!({
        "result": {
            "tools": TOOLS.iter().map(|tool| tool.to_json()).collect::<Vec<_>>()
        }
    })
}

/// Build the `resources/list` response.
pub(crate) fn resources_list_json() -> Value {
    json!({
        "result": {
            "resources": RESOURCES.iter().map(|resource| resource.to_json()).collect::<Vec<_>>()
        }
    })
}

/// Build the `resources/templates/list` response.
pub(crate) fn resource_templates_list_json() -> Value {
    json!({
        "result": {
            "resourceTemplates": RESOURCE_TEMPLATES
                .iter()
                .map(|template| template.to_json())
                .collect::<Vec<_>>()
        }
    })
}

/// Build the `prompts/list` response.
pub(crate) fn prompts_list_json() -> Value {
    json!({
        "result": {
            "prompts": PROMPTS.iter().map(|prompt| prompt.list_json()).collect::<Vec<_>>()
        }
    })
}

/// Build the `prompts/get` response or fail when the prompt is unknown.
pub(crate) fn prompt_get_json(name: &str) -> Result<Value> {
    PROMPTS
        .iter()
        .find(|prompt| prompt.name == name)
        .map(|prompt| prompt.get_json())
        .ok_or_else(|| anyhow::anyhow!("Unknown prompt: {name}"))
}

fn debug_assert_read_only_catalog() {
    debug_assert!(
        TOOLS
            .iter()
            .all(|tool| matches!(tool.safety.effect, ToolEffect::Read)),
        "stdio catalog may only expose read-only tools"
    );
}
