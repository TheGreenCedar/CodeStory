//! Catalog for the stdio integration surface.
//!
//! This module defines the tool/resource/prompt metadata exposed by
//! `serve --stdio`. The catalog is declarative JSON shape: keep names, schemas,
//! annotations, and safety metadata stable because clients discover behavior
//! from these responses before calling into the transport.

use anyhow::Result;
use codestory_contracts::api::{PACKET_PROBE_MAX_COUNT, PACKET_PROBE_MAX_TEXT_LENGTH};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy)]
/// Safety metadata emitted in both legacy and annotation-style catalog fields.
pub(crate) struct SafetyMetadata {
    activates_managed_state: bool,
}

impl SafetyMetadata {
    pub(crate) const fn observational() -> Self {
        Self {
            activates_managed_state: false,
        }
    }

    pub(crate) const fn managed_activation() -> Self {
        Self {
            activates_managed_state: true,
        }
    }

    fn to_json(self) -> Value {
        json!({
            "effect": if self.activates_managed_state { "managed_activation" } else { "read_only" },
            "readOnly": !self.activates_managed_state,
            "sideEffects": self.activates_managed_state,
            "activatesProject": self.activates_managed_state,
            "writesRepository": false,
            "destructive": false,
            "idempotent": true,
            "requiresConfirmation": false,
            "localOnly": !self.activates_managed_state,
            "openWorld": self.activates_managed_state
        })
    }

    fn annotations_json(self) -> Value {
        json!({
            "readOnlyHint": !self.activates_managed_state,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": self.activates_managed_state
        })
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
        let mut input_schema = self.input_schema.to_json();
        let input = input_schema
            .as_object_mut()
            .expect("stdio tool input schema must be an object");
        input
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .expect("stdio tool properties must be an object")
            .insert(
                "project".to_string(),
                SchemaProperty::string_required(
                    "project",
                    "Absolute repository root for this request. The MCP server is multi-project and does not retain a global workspace binding.",
                )
                .with_min_length(1)
                .to_json(),
            );
        input
            .get_mut("required")
            .and_then(Value::as_array_mut)
            .expect("stdio tool required list must be an array")
            .push(json!("project"));
        let mut tool = Map::from_iter([
            ("name".to_string(), json!(self.name)),
            ("description".to_string(), json!(self.description)),
            ("inputSchema".to_string(), input_schema),
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
    OneOf(&'static [&'static SchemaObject]),
    Type {
        schema_type: SchemaType,
        min_length: Option<u64>,
        max_length: Option<u64>,
    },
}

impl SchemaItems {
    fn to_json(self) -> Value {
        match self {
            Self::Object(object) => object.to_json(),
            Self::OneOf(variants) => json!({
                "oneOf": variants
                    .iter()
                    .map(|variant| variant.to_json())
                    .collect::<Vec<_>>()
            }),
            Self::Type {
                schema_type,
                min_length,
                max_length,
            } => {
                let mut schema =
                    Map::from_iter([("type".to_string(), json!(schema_type.as_str()))]);
                if let Some(min_length) = min_length {
                    schema.insert("minLength".to_string(), json!(min_length));
                }
                if let Some(max_length) = max_length {
                    schema.insert("maxLength".to_string(), json!(max_length));
                }
                Value::Object(schema)
            }
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
    max_length: Option<u64>,
    min_items: Option<u64>,
    max_items: Option<u64>,
    items: Option<SchemaItems>,
    object_schema: Option<&'static SchemaObject>,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: None,
            object_schema: None,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: None,
            object_schema: None,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: None,
            object_schema: None,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: None,
            object_schema: None,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: None,
            object_schema: None,
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: Some(SchemaItems::Object(items)),
            object_schema: None,
            nullable: false,
        }
    }

    const fn tagged_union_array(
        name: &'static str,
        description: &'static str,
        variants: &'static [&'static SchemaObject],
    ) -> Self {
        let mut property = Self::array(name, description, variants[0]);
        property.items = Some(SchemaItems::OneOf(variants));
        property
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
            max_length: None,
            min_items: None,
            max_items: None,
            items: Some(SchemaItems::Type {
                schema_type: SchemaType::String,
                min_length: None,
                max_length: None,
            }),
            object_schema: None,
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

    const fn with_max_length(mut self, max_length: u64) -> Self {
        self.max_length = Some(max_length);
        self
    }

    const fn with_item_bounds(mut self, min_items: u64, max_items: u64) -> Self {
        self.min_items = Some(min_items);
        self.max_items = Some(max_items);
        self
    }

    const fn with_item_min_length(mut self, min_length: u64) -> Self {
        self.items = match self.items {
            Some(SchemaItems::Type {
                schema_type,
                max_length,
                ..
            }) => Some(SchemaItems::Type {
                schema_type,
                min_length: Some(min_length),
                max_length,
            }),
            items => items,
        };
        self
    }

    const fn with_item_max_length(mut self, max_length: u64) -> Self {
        self.items = match self.items {
            Some(SchemaItems::Type {
                schema_type,
                min_length,
                ..
            }) => Some(SchemaItems::Type {
                schema_type,
                min_length,
                max_length: Some(max_length),
            }),
            items => items,
        };
        self
    }

    const fn with_object_schema(mut self, object_schema: &'static SchemaObject) -> Self {
        self.object_schema = Some(object_schema);
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
        if let Some(max_length) = self.max_length {
            schema.insert("maxLength".to_string(), json!(max_length));
        }
        if let Some(min_items) = self.min_items {
            schema.insert("minItems".to_string(), json!(min_items));
        }
        if let Some(max_items) = self.max_items {
            schema.insert("maxItems".to_string(), json!(max_items));
        }
        if let Some(items) = self.items {
            schema.insert("items".to_string(), items.to_json());
        }
        if let Some(object_schema) = self.object_schema {
            let nested_schema = object_schema.to_json();
            if let Some(nested) = nested_schema.as_object() {
                for (key, value) in nested {
                    if key != "type" && key != "description" {
                        schema.insert(key.clone(), value.clone());
                    }
                }
            }
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
    one_of_required: &'static [&'static [&'static str]],
    combined_item_limit: Option<(&'static str, &'static str, u64)>,
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
            one_of_required: &[],
            combined_item_limit: None,
            additional_properties: false,
        }
    }

    const fn passthrough_object(description: &'static str) -> Self {
        Self {
            description: Some(description),
            properties: &[],
            required: &[],
            any_of_required: &[],
            one_of_required: &[],
            combined_item_limit: None,
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

    const fn with_one_of_required(
        mut self,
        one_of_required: &'static [&'static [&'static str]],
    ) -> Self {
        self.one_of_required = one_of_required;
        self
    }

    const fn with_combined_item_limit(
        mut self,
        left: &'static str,
        right: &'static str,
        limit: u64,
    ) -> Self {
        self.combined_item_limit = Some((left, right, limit));
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
        if !self.one_of_required.is_empty() {
            schema.insert(
                "oneOf".to_string(),
                Value::Array(
                    self.one_of_required
                        .iter()
                        .map(|required| json!({ "required": required }))
                        .collect(),
                ),
            );
        }
        if let Some((left, right, limit)) = self.combined_item_limit {
            schema.insert(
                "allOf".to_string(),
                Value::Array(
                    (1..=limit)
                        .map(|left_minimum| {
                            json!({
                                "not": {
                                    "required": [left, right],
                                    "properties": {
                                        (left): {"minItems": left_minimum},
                                        (right): {"minItems": limit + 1 - left_minimum}
                                    }
                                }
                            })
                        })
                        .collect(),
                ),
            );
        }
        Value::Object(schema)
    }
}

const TEXT_HIT_ORIGINS: &[&str] = &["indexed_symbol", "text_match"];
const PACKET_EVIDENCE_TIERS: &[&str] = &[
    "exact_source",
    "structural_text",
    "resolved_graph",
    "lexical_source",
    "symbol_doc",
    "component_report",
    "dense_semantic",
    "synthetic_source_scan",
    "generated_summary",
];
const PACKET_EVIDENCE_RESOLUTIONS: &[&str] = &[
    "resolved",
    "source_range_only",
    "unresolved",
    "diagnostic_only",
];
const SEARCH_REPO_TEXT_MODES: &[&str] = &["auto", "on", "off"];
const INDEXED_FILE_ROLES: &[&str] = &["source", "test", "generated", "vendor", "unknown"];
const SNIPPET_SCOPES: &[&str] = &["line_context", "function_body"];
const GROUNDING_BUDGETS: &[&str] = &["strict", "balanced", "max"];
const PACKET_BUDGETS: &[&str] = &["tiny", "compact", "standard", "deep"];
const PACKET_PROBE_EXACT_PATH_KIND: &[&str] = &["exact_path"];
const PACKET_PROBE_SYMBOL_ID_KIND: &[&str] = &["symbol_id"];
const PACKET_PROBE_FILE_SYMBOL_KIND: &[&str] = &["file_symbol"];
const PACKET_PROBE_FREE_QUERY_KIND: &[&str] = &["free_query"];
const PACKET_PROBE_CONTINUATION_KIND: &[&str] = &["continuation"];
const AFFECTED_CHANGE_KINDS: &[&str] = &[
    "added",
    "modified",
    "deleted",
    "renamed",
    "copied",
    "untracked",
    "unknown",
];
const AFFECTED_INPUT_CLASSIFICATIONS: &[&str] = &[
    "valid_uncovered",
    "missing",
    "expected_deleted",
    "rename_unresolved",
    "stale_index",
    "malformed",
    "unavailable_evidence",
];
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

static STATUS_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Compact capability state. Read codestory://status only when full diagnostics are needed.",
    &[
        SchemaProperty::string("project", "Requested repository root."),
        SchemaProperty::string("state", "Overall capability state.").with_enum(&[
            "ready",
            "preparing",
            "updating",
            "working_locally",
            "unavailable",
        ]),
        SchemaProperty::object("capabilities", "Local navigation and broad-search states."),
        SchemaProperty::object(
            "current_operation",
            "Current managed preparation operation.",
        )
        .nullable(),
        SchemaProperty::string("next_action", "Direct next action for the caller."),
        SchemaProperty::integer("retry_after_ms", "Retry delay while preparing.").nullable(),
        SchemaProperty::string("diagnostics_uri", "Optional full diagnostic resource URI."),
    ],
    &[
        "project",
        "state",
        "capabilities",
        "next_action",
        "diagnostics_uri",
    ],
);

static RESOURCE_LINK_SCHEMA: SchemaObject = SchemaObject::object(
    "Continuation resource link.",
    &[
        SchemaProperty::string("rel", "Link relation."),
        SchemaProperty::string("uri", "CodeStory resource URI."),
        SchemaProperty::object(
            "probe",
            "Optional generation-bound continuation probe for packet reuse.",
        ),
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
        SchemaProperty::string(
            "evidence_tier",
            "Evidence provenance tier. structural_text is collector-backed source-range evidence, not parser-backed graph coverage.",
        )
        .with_enum(PACKET_EVIDENCE_TIERS),
        SchemaProperty::string(
            "evidence_producer",
            "Collector or retrieval producer that emitted the evidence.",
        ),
        SchemaProperty::string(
            "resolution_status",
            "Resolution state. source_range_only has a source span but no typed graph resolution.",
        )
        .with_enum(PACKET_EVIDENCE_RESOLUTIONS),
        SchemaProperty::boolean(
            "eligible_for_sufficiency",
            "Whether this hit may satisfy answer-sufficiency requirements.",
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
        SchemaProperty::object("retrieval", "Retrieval readiness."),
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
            "repo_text_stats",
            "Repo text scan cap, byte, and truncation telemetry.",
        )
        .nullable(),
        SchemaProperty::object("counts", "Source counts before merged-result deduplication."),
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
    &[
        SchemaProperty::array(
            "symbols",
            "Root or child symbol summaries.",
            &SYMBOL_SUMMARY_SCHEMA,
        ),
        SchemaProperty::integer("returned_count", "Symbol rows included in this response."),
        SchemaProperty::integer("limit", "Applied result limit."),
        SchemaProperty::boolean("truncated", "Whether matching symbols exceeded the limit."),
    ],
    &["symbols", "returned_count", "limit", "truncated"],
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

static AFFECTED_CHANGE_RECORD_SCHEMA: SchemaObject = SchemaObject::object(
    "Changed file record.",
    &[
        SchemaProperty::string_required("path", "Changed repo-relative path.").with_min_length(1),
        SchemaProperty::string("kind", "Change kind.").with_enum(AFFECTED_CHANGE_KINDS),
        SchemaProperty::string(
            "status",
            "Optional raw git-style status such as M, A, D, R100, C100, or ??.",
        ),
        SchemaProperty::string(
            "previous_path",
            "Optional previous path accepted only for renamed or copied records; it can seed bounded proxy graph evidence when the current path is not indexed.",
        )
        .nullable(),
    ],
    &["path", "kind"],
);

static AFFECTED_MATCHED_FILE_SCHEMA: SchemaObject = SchemaObject::object(
    "Matched indexed file row.",
    &[
        SchemaProperty::string("path", "Project-relative file path."),
        SchemaProperty::string("role", "Inferred file role.").with_enum(INDEXED_FILE_ROLES),
        SchemaProperty::boolean("indexed", "Whether the file was indexed."),
        SchemaProperty::boolean("complete", "Whether indexing completed for this file."),
        SchemaProperty::string("change_kind", "Matched change kind.")
            .with_enum(AFFECTED_CHANGE_KINDS)
            .nullable(),
        SchemaProperty::string("change_status", "Matched raw change status.").nullable(),
        SchemaProperty::string("previous_path", "Previous rename/copy path.").nullable(),
        SchemaProperty::integer("error_count", "File-level index error count."),
    ],
    &["path", "role", "indexed", "complete", "error_count"],
);

static AFFECTED_UNMATCHED_PATH_SCHEMA: SchemaObject = SchemaObject::object(
    "Input path that did not match indexed file identity.",
    &[
        SchemaProperty::string("path", "Submitted project-relative path."),
        SchemaProperty::string(
            "classification",
            "Positive-evidence coverage classification.",
        )
        .with_enum(AFFECTED_INPUT_CLASSIFICATIONS),
        SchemaProperty::string("reason", "Human-readable classification reason."),
        SchemaProperty::string_array("evidence", "Evidence supporting the classification."),
        SchemaProperty::string("change_kind", "Submitted change kind.")
            .with_enum(AFFECTED_CHANGE_KINDS)
            .nullable(),
        SchemaProperty::string("change_status", "Submitted raw change status.").nullable(),
        SchemaProperty::string("previous_path", "Previous rename/copy path.").nullable(),
    ],
    &["path", "classification", "reason", "evidence"],
);

static AFFECTED_UNCOVERED_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Input without complete graph evidence.",
    &[
        SchemaProperty::string("path", "Submitted project-relative path."),
        SchemaProperty::string("classification", "Evidence-backed coverage classification.")
            .with_enum(AFFECTED_INPUT_CLASSIFICATIONS),
        SchemaProperty::string("reason", "Human-readable classification reason."),
        SchemaProperty::string_array("evidence", "Evidence supporting the classification."),
    ],
    &["path", "classification", "reason", "evidence"],
);

static AFFECTED_BOUNDS_SCHEMA: SchemaObject = SchemaObject::object(
    "Applied traversal and result bounds.",
    &[
        SchemaProperty::integer("requested_depth", "Applied dependent graph walk depth."),
        SchemaProperty::integer("maximum_depth", "Maximum allowed graph walk depth."),
        SchemaProperty::integer("visited_node_count", "Visited graph node count."),
        SchemaProperty::integer("visited_edge_count", "Visited graph edge count."),
        SchemaProperty::integer("impacted_symbol_limit", "Runtime impacted-symbol limit."),
        SchemaProperty::integer("impacted_route_limit", "Runtime impacted-route limit."),
    ],
    &[
        "requested_depth",
        "maximum_depth",
        "visited_node_count",
        "visited_edge_count",
        "impacted_symbol_limit",
        "impacted_route_limit",
    ],
);

static AFFECTED_COMPLETENESS_SCHEMA: SchemaObject = SchemaObject::object(
    "Completeness and truncation evidence for this response.",
    &[
        SchemaProperty::boolean("complete", "Whether a complete impact claim is supported."),
        SchemaProperty::string("confidence", "Completeness confidence."),
        SchemaProperty::integer("direct_impact_count", "Direct impacted-symbol count."),
        SchemaProperty::integer(
            "propagated_impact_count",
            "Graph-propagated impacted-symbol count.",
        ),
        SchemaProperty::integer("candidate_test_count", "Candidate impacted-test count."),
        SchemaProperty::integer(
            "uncovered_input_count",
            "Inputs without complete graph evidence.",
        ),
        SchemaProperty::integer(
            "unavailable_evidence_count",
            "Inputs whose absence could not be classified more strongly.",
        ),
        SchemaProperty::boolean(
            "truncated",
            "Whether runtime or transport bounds capped evidence.",
        ),
        SchemaProperty::string_array(
            "truncation_reasons",
            "Field-specific runtime and transport truncation reasons.",
        ),
    ],
    &[
        "complete",
        "confidence",
        "direct_impact_count",
        "propagated_impact_count",
        "candidate_test_count",
        "uncovered_input_count",
        "unavailable_evidence_count",
        "truncated",
        "truncation_reasons",
    ],
);

static AFFECTED_FOLLOW_UP_INVOCATION_SCHEMA: SchemaObject = SchemaObject::object(
    "Structured follow-up invocation rendered only by a client.",
    &[
        SchemaProperty::string("program", "Executable name."),
        SchemaProperty::string_array("args", "Unquoted argument vector."),
    ],
    &["program", "args"],
);

static AFFECTED_FOLLOW_UP_SCHEMA: SchemaObject = SchemaObject::object(
    "Evidence-derived follow-up action.",
    &[
        SchemaProperty::string("action", "Stable follow-up action label."),
        SchemaProperty::string("reason", "Evidence-backed reason for the follow-up."),
        SchemaProperty::string("confidence", "Follow-up confidence."),
        SchemaProperty::object("invocation", "Optional structured command invocation.")
            .with_object_schema(&AFFECTED_FOLLOW_UP_INVOCATION_SCHEMA),
    ],
    &["action", "reason", "confidence"],
);

static AFFECTED_ANALYSIS_OUTPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Changed-file impact analysis DTO from the last complete local index.",
    &[
        SchemaProperty::string("project_root", "Project root."),
        SchemaProperty::string_array("changed_paths", "Changed repo-relative paths."),
        SchemaProperty::array(
            "change_records",
            "Normalized changed file records.",
            &AFFECTED_CHANGE_RECORD_SCHEMA,
        ),
        SchemaProperty::array(
            "matched_files",
            "Changed paths matched to indexed files.",
            &AFFECTED_MATCHED_FILE_SCHEMA,
        ),
        SchemaProperty::array(
            "unmatched_paths",
            "Changed paths that did not match indexed file identity.",
            &AFFECTED_UNMATCHED_PATH_SCHEMA,
        ),
        SchemaProperty::array(
            "uncovered_inputs",
            "All inputs without complete graph evidence, including malformed indexed files.",
            &AFFECTED_UNCOVERED_INPUT_SCHEMA,
        ),
        SchemaProperty::integer("matched_file_count", "Number of matched indexed files."),
        SchemaProperty::integer("depth", "Applied dependent graph walk depth."),
        SchemaProperty::array(
            "impacted_symbols",
            "Impacted symbol DTOs.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::array(
            "impacted_routes",
            "Impacted route or endpoint DTOs.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::array(
            "impacted_tests",
            "Likely impacted test file DTOs.",
            &GENERIC_OBJECT_SCHEMA,
        ),
        SchemaProperty::object("bounds", "Applied traversal and result bounds.")
            .with_object_schema(&AFFECTED_BOUNDS_SCHEMA),
        SchemaProperty::object(
            "completeness",
            "Completeness, direct/propagated counts, confidence, and truncation evidence.",
        )
        .with_object_schema(&AFFECTED_COMPLETENESS_SCHEMA),
        SchemaProperty::string_array("blind_spots", "Known impact-analysis blind spots."),
        SchemaProperty::array(
            "follow_ups",
            "Evidence-derived follow-up actions with optional structured invocations.",
            &AFFECTED_FOLLOW_UP_SCHEMA,
        ),
        SchemaProperty::string_array("notes", "Additional analysis notes."),
        SchemaProperty::object("counts", "Original result counts before response caps."),
        SchemaProperty::object("limits", "Applied response caps."),
        SchemaProperty::boolean("truncated", "Whether any result collection was capped."),
        SchemaProperty::string("code", "Typed API error code."),
        SchemaProperty::string("message", "Human-readable API error message."),
        SchemaProperty::object("details", "Structured API error repair guidance.").nullable(),
    ],
    &[],
)
.with_any_of_required(&[
    &[
        "project_root",
        "changed_paths",
        "change_records",
        "matched_files",
        "uncovered_inputs",
        "matched_file_count",
        "depth",
        "impacted_symbols",
        "impacted_tests",
        "bounds",
        "completeness",
    ],
    &["code", "message"],
]);

static GROUNDING_SNAPSHOT_SCHEMA: SchemaObject = SchemaObject::object(
    "CodeStory grounding snapshot DTO for compact repository orientation.",
    &[
        SchemaProperty::string("root", "Project root."),
        SchemaProperty::string("budget", "Grounding output budget.").with_enum(GROUNDING_BUDGETS),
        SchemaProperty::integer("generated_at_epoch_ms", "Snapshot generation time."),
        SchemaProperty::object("stats", "Indexed project stats."),
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
        SchemaProperty::string(
            "range_source",
            "Source of the selected function-body range, when available.",
        ),
        SchemaProperty::string(
            "fallback_reason",
            "Reason function-body selection fell back to line context, when applicable.",
        ),
        SchemaProperty::string(
            "truncation_guidance",
            "Follow-up guidance when the snippet hit its byte cap.",
        ),
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
        SchemaProperty::string(
            "evidence_tier",
            "Evidence provenance tier. structural_text is collector-backed source-range evidence, not parser-backed graph coverage.",
        )
        .with_enum(PACKET_EVIDENCE_TIERS),
        SchemaProperty::string(
            "evidence_producer",
            "Collector or retrieval producer that emitted the evidence.",
        ),
        SchemaProperty::string(
            "resolution_status",
            "Resolution state. source_range_only has a source span but no typed graph resolution.",
        )
        .with_enum(PACKET_EVIDENCE_RESOLUTIONS),
        SchemaProperty::boolean(
            "eligible_for_sufficiency",
            "Whether this citation may satisfy answer-sufficiency requirements.",
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
        SchemaProperty::integer("max_nodes", "Maximum graph nodes returned.")
            .with_default(ValueLiteral::Integer(120))
            .with_bounds(1, 120),
        SchemaProperty::boolean("story", "Include a readable trail story DTO.")
            .with_default(ValueLiteral::Boolean(false)),
    ],
    &[],
)
.with_any_of_required(&[&["query"], &["id"]]);

static LOCAL_GRAPH_ALIAS_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a bounded local graph alias around one node.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
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

static TRACE_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Return a readable trace around a symbol id or query.",
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
        SchemaProperty::integer("max_nodes", "Maximum graph nodes returned.")
            .with_default(ValueLiteral::Integer(120))
            .with_bounds(1, 120),
        SchemaProperty::boolean("story", "Include a readable trail story DTO.")
            .with_default(ValueLiteral::Boolean(true)),
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

static SNIPPET_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Resolve a symbol and return bounded line or function-body source context.",
    &[
        SchemaProperty::string("query", "Symbol query.").with_min_length(1),
        SchemaProperty::string("id", "Stable node id.").with_min_length(1),
        SchemaProperty::integer(
            "choose",
            "Resolve by the 1-based alternative number from an ambiguity error.",
        )
        .with_bounds(1, 50),
        SchemaProperty::string("scope", "Snippet scope.")
            .with_enum(SNIPPET_SCOPES)
            .with_default(ValueLiteral::String("line_context")),
        SchemaProperty::integer(
            "context",
            "Surrounding context lines above and below the selected source range.",
        )
        .with_default(ValueLiteral::Integer(4))
        .with_bounds(0, 200),
        SchemaProperty::integer(
            "lines",
            "Agent-friendly compatibility alias for `context`.",
        )
        .with_bounds(0, 200),
        SchemaProperty::boolean(
            "function_body",
            "CLI-compatible scope selector; true requests `function_body`, false requests `line_context`.",
        ),
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
            .with_default(ValueLiteral::Integer(100))
            .with_bounds(1, 500),
    ],
    &[],
);

static AFFECTED_INPUT_SCHEMA: SchemaObject = SchemaObject::object(
    "Analyze exactly one explicit path source against the last complete local index.",
    &[
        SchemaProperty::string_array(
            "paths",
            "Preferred simple input: project-relative paths to analyze.",
        )
        .with_item_min_length(1)
        .with_item_bounds(1, 200),
        SchemaProperty::string_array(
            "changed_paths",
            "Compatibility alias for project-relative paths to analyze.",
        )
        .with_item_min_length(1)
        .with_item_bounds(1, 200),
        SchemaProperty::array(
            "change_records",
            "Changed file records with path, kind, optional status, and optional previous_path.",
            &AFFECTED_CHANGE_RECORD_SCHEMA,
        )
        .with_item_bounds(1, 200),
        SchemaProperty::integer("depth", "Dependent graph walk depth.")
            .with_default(ValueLiteral::Integer(2))
            .with_bounds(1, 8),
        SchemaProperty::string(
            "filter",
            "Optional impacted-symbol filter by path or display-name substring.",
        ),
    ],
    &[],
)
.with_one_of_required(&[&["paths"], &["changed_paths"], &["change_records"]]);

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

static PACKET_EXACT_PATH_PROBE_SCHEMA: SchemaObject = SchemaObject::object(
    "Exact project-relative path probe.",
    &[
        SchemaProperty::string_required("kind", "Probe kind.")
            .with_enum(PACKET_PROBE_EXACT_PATH_KIND),
        SchemaProperty::string_required("path", "Exact project-relative path.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
    ],
    &["kind", "path"],
);

static PACKET_SYMBOL_ID_PROBE_SCHEMA: SchemaObject = SchemaObject::object(
    "Stable symbol-id probe.",
    &[
        SchemaProperty::string_required("kind", "Probe kind.")
            .with_enum(PACKET_PROBE_SYMBOL_ID_KIND),
        SchemaProperty::string_required("id", "Stable symbol id.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
    ],
    &["kind", "id"],
);

static PACKET_FILE_SYMBOL_PROBE_SCHEMA: SchemaObject = SchemaObject::object(
    "Exact file-scoped symbol probe.",
    &[
        SchemaProperty::string_required("kind", "Probe kind.")
            .with_enum(PACKET_PROBE_FILE_SYMBOL_KIND),
        SchemaProperty::string_required("path", "Exact project-relative path.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
        SchemaProperty::string_required("symbol", "File-scoped symbol name.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
    ],
    &["kind", "path", "symbol"],
);

static PACKET_FREE_QUERY_PROBE_SCHEMA: SchemaObject = SchemaObject::object(
    "Free-query probe.",
    &[
        SchemaProperty::string_required("kind", "Probe kind.")
            .with_enum(PACKET_PROBE_FREE_QUERY_KIND),
        SchemaProperty::string_required("query", "Free query.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
    ],
    &["kind", "query"],
);

static PACKET_CONTINUATION_PROBE_SCHEMA: SchemaObject = SchemaObject::object(
    "Project- and generation-bound continuation probe.",
    &[
        SchemaProperty::string_required("kind", "Probe kind.")
            .with_enum(PACKET_PROBE_CONTINUATION_KIND),
        SchemaProperty::integer("contract_version", "Continuation probe contract version.")
            .with_bounds(1, 1),
        SchemaProperty::string_required("project_id", "Continuation project identity.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
        SchemaProperty::string_required(
            "core_generation_id",
            "Continuation core evidence generation.",
        )
        .with_min_length(1)
        .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
        SchemaProperty::string(
            "retrieval_generation",
            "Optional continuation retrieval generation.",
        )
        .with_min_length(1)
        .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64)
        .nullable(),
        SchemaProperty::string("symbol_id", "Optional exact continuation symbol id.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64)
            .nullable(),
        SchemaProperty::string_required("query", "Continuation display query.")
            .with_min_length(1)
            .with_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
    ],
    &[
        "kind",
        "contract_version",
        "project_id",
        "core_generation_id",
        "query",
    ],
);

static PACKET_PROBE_SCHEMAS: &[&SchemaObject] = &[
    &PACKET_EXACT_PATH_PROBE_SCHEMA,
    &PACKET_SYMBOL_ID_PROBE_SCHEMA,
    &PACKET_FILE_SYMBOL_PROBE_SCHEMA,
    &PACKET_FREE_QUERY_PROBE_SCHEMA,
    &PACKET_CONTINUATION_PROBE_SCHEMA,
];

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
        SchemaProperty::tagged_union_array(
            "probes",
            "Optional tagged exact-path, symbol-id, file-symbol, free-query, or generation-bound continuation probes.",
            PACKET_PROBE_SCHEMAS,
        )
        .with_item_bounds(1, PACKET_PROBE_MAX_COUNT as u64),
        SchemaProperty::string_array(
            "extra_probes",
            "Legacy string probes normalized through the same typed runtime resolver.",
        )
        .with_item_bounds(1, PACKET_PROBE_MAX_COUNT as u64)
        .with_item_min_length(1)
        .with_item_max_length(PACKET_PROBE_MAX_TEXT_LENGTH as u64),
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
)
.with_combined_item_limit("probes", "extra_probes", PACKET_PROBE_MAX_COUNT as u64);

static STATUS_INPUT_SCHEMA: SchemaObject =
    SchemaObject::object("Read readiness for one explicit repository.", &[], &[]);

static TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "status",
        description: "Inspect CodeStory readiness for the requested repository when diagnostics are needed.",
        input_schema: STATUS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(STATUS_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::observational(),
    },
    ToolSpec {
        name: "packet",
        description: "Answer broad structural questions with repository evidence, sufficiency, truncation, and follow-up commands before source snippets. CodeStory prepares managed retrieval automatically.",
        input_schema: PACKET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(AGENT_PACKET_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "search",
        description: "Discover candidate symbols and retrieval hits; for broad structural questions call packet before snippet/source reads. CodeStory prepares managed retrieval automatically.",
        input_schema: SEARCH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SEARCH_RESULTS_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "ground",
        description: "Return a compact repository map for orientation before packet/search; equivalent to codestory://grounding. The first call may refresh the local map and begin managed retrieval preparation.",
        input_schema: GROUND_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GROUNDING_SNAPSHOT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "files",
        description: "List indexed files and coverage from a locally fresh index; refreshes the repository map before dispatch and does not wait for broad search.",
        input_schema: FILES_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(INDEXED_FILES_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "affected",
        description: "Analyze one explicit path source against the last complete local index while preserving bounded stale and error evidence. Cold or partial state may trigger managed indexing before dispatch. Prefer paths, use changed_paths for compatibility or change_records for status-rich input. Never discovers git changes and does not wait for broad search.",
        input_schema: AFFECTED_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(AFFECTED_ANALYSIS_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "symbol",
        description: "Resolve a symbol id or query and return details.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SYMBOL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "trail",
        description: "Return a graph trail around a symbol.",
        input_schema: TRAIL_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(TRAIL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "callers",
        description: "Return a bounded incoming caller graph around a symbol.",
        input_schema: LOCAL_GRAPH_ALIAS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "callees",
        description: "Return a bounded outgoing callee graph around a symbol.",
        input_schema: LOCAL_GRAPH_ALIAS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "trace",
        description: "Return a readable trace around a symbol.",
        input_schema: TRACE_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(TRAIL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "get_node",
        description: "Return one stable graph node with file refs before requesting a packet.",
        input_schema: GRAPH_TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "neighbors",
        description: "Return a bounded graph neighborhood around one node.",
        input_schema: GRAPH_NEIGHBORS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "shortest_path",
        description: "Return a bounded forward path graph between two node ids.",
        input_schema: SHORTEST_PATH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "query_subgraph",
        description: "Return a bounded subgraph around one resolved node; packet remains the broad task tool.",
        input_schema: QUERY_SUBGRAPH_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(GRAPH_TOOL_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "definition",
        description: "Return definition metadata for a symbol id or query.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(DEFINITION_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "references",
        description: "Return incoming references for a symbol id or query.",
        input_schema: TARGET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(TRAIL_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "symbols",
        description: "Browse root symbols or children for a parent id.",
        input_schema: SYMBOLS_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SYMBOLS_OUTPUT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "snippet",
        description: "Return a focused source snippet after packet, search, or graph evidence selects a concrete target.",
        input_schema: SNIPPET_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(SNIPPET_CONTEXT_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
    ToolSpec {
        name: "context",
        description: "Build proof-bearing source/graph evidence for one concrete target; not broad question answering.",
        input_schema: CONTEXT_INPUT_SCHEMA,
        output_schema: Some(SchemaSpec::Object(CONTEXT_PACKET_SCHEMA)),
        safety: SafetyMetadata::managed_activation(),
    },
];

static RESOURCES: &[ResourceSpec] = &[ResourceSpec {
    uri: "codestory://agent-guide",
    name: "Agent guide",
    mime_type: "application/json",
}];

static RESOURCE_TEMPLATES: &[ResourceTemplateSpec] = &[
    ResourceTemplateSpec {
        uri_template: "codestory://status{?project}",
        name: "Status",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://project{?project}",
        name: "Project summary",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://grounding{?project}",
        name: "Grounding snapshot",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://symbols/root{?project}",
        name: "Root symbols",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://symbol/{node_id}{?project}",
        name: "Symbol details",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://references/{node_id}{?project}",
        name: "Symbol references",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://snippet/{node_id}{?project}",
        name: "Symbol snippet",
        mime_type: "application/json",
    },
    ResourceTemplateSpec {
        uri_template: "codestory://trail/{node_id}{?project}",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_probe_schema() -> Value {
        let catalog = tools_list_json();
        catalog["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == "packet")
            .expect("packet tool")["inputSchema"]["properties"]["probes"]["items"]
            .clone()
    }

    fn tagged_union_accepts(schema: &Value, value: &Value) -> bool {
        let Some(value) = value.as_object() else {
            return false;
        };
        schema["oneOf"]
            .as_array()
            .expect("probe oneOf")
            .iter()
            .filter(|variant| {
                let properties = variant["properties"].as_object().expect("properties");
                let required = variant["required"].as_array().expect("required");
                if value.keys().any(|key| !properties.contains_key(key))
                    || required
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|field| !value.contains_key(field))
                {
                    return false;
                }
                value.iter().all(|(field, field_value)| {
                    let property = &properties[field];
                    if let Some(allowed) = property["enum"].as_array()
                        && !allowed.contains(field_value)
                    {
                        return false;
                    }
                    if let Some(text) = field_value.as_str() {
                        let length = text.chars().count() as u64;
                        if property["minLength"]
                            .as_u64()
                            .is_some_and(|minimum| length < minimum)
                            || property["maxLength"]
                                .as_u64()
                                .is_some_and(|maximum| length > maximum)
                        {
                            return false;
                        }
                    }
                    true
                })
            })
            .count()
            == 1
    }

    #[test]
    fn packet_probe_schema_is_a_strict_bounded_tagged_union() {
        let schema = packet_probe_schema();
        assert_eq!(schema["oneOf"].as_array().map(Vec::len), Some(5));
        for valid in [
            json!({"kind": "exact_path", "path": "assets/desk.svg"}),
            json!({"kind": "symbol_id", "id": "42"}),
            json!({"kind": "file_symbol", "path": "src/lib.rs", "symbol": "run"}),
            json!({"kind": "free_query", "query": "runtime path"}),
            json!({
                "kind": "continuation",
                "contract_version": 1,
                "project_id": "project",
                "core_generation_id": "core",
                "query": "run"
            }),
        ] {
            assert!(tagged_union_accepts(&schema, &valid), "{valid}");
        }
        for invalid in [
            json!({"kind": "exact_path"}),
            json!({"kind": "exact_path", "path": "src/lib.rs", "query": "extra"}),
            json!({"kind": "file_symbol", "path": "src/lib.rs"}),
            json!({"kind": "free_query", "query": ""}),
            json!({
                "kind": "free_query",
                "query": "x".repeat(PACKET_PROBE_MAX_TEXT_LENGTH + 1)
            }),
        ] {
            assert!(!tagged_union_accepts(&schema, &invalid), "{invalid}");
        }

        let catalog = tools_list_json();
        let packet = catalog["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == "packet")
            .expect("packet tool");
        assert_eq!(
            packet["inputSchema"]["allOf"].as_array().map(Vec::len),
            Some(PACKET_PROBE_MAX_COUNT)
        );
    }
}
