use codestory_contracts::graph::NodeKind;
use std::collections::HashSet;

pub(crate) fn semantic_doc_language_from_path(path: Option<&str>) -> Option<&'static str> {
    let ext = path?
        .rsplit('.')
        .next()?
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match ext.as_str() {
        "c" => Some("c"),
        "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" => Some("cpp"),
        "java" => Some("java"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "py" | "pyi" => Some("python"),
        "rs" => Some("rust"),
        "ts" | "tsx" | "mts" | "cts" => Some("typescript"),
        _ => None,
    }
}

pub(crate) fn semantic_symbol_role_aliases(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "module namespace package container",
        NodeKind::NAMESPACE => "namespace module package container",
        NodeKind::PACKAGE => "package module namespace container",
        NodeKind::FILE => "file source module unit",
        NodeKind::STRUCT => "struct record data type model",
        NodeKind::CLASS => "class object type model",
        NodeKind::INTERFACE => "interface protocol contract abstraction",
        NodeKind::ANNOTATION => "annotation decorator attribute metadata",
        NodeKind::UNION => "union variant data type",
        NodeKind::ENUM => "enum enumeration choices variants",
        NodeKind::TYPEDEF => "typedef type alias named type",
        NodeKind::TYPE_PARAMETER => "type parameter generic parameter",
        NodeKind::BUILTIN_TYPE => "builtin type primitive",
        NodeKind::FUNCTION => "function callable routine procedure operation",
        NodeKind::METHOD => "method member function object behavior callable routine operation",
        NodeKind::MACRO => "macro compile time expansion preprocessor metaprogramming",
        NodeKind::GLOBAL_VARIABLE => "global variable shared state value",
        NodeKind::FIELD => "field property member attribute",
        NodeKind::VARIABLE => "variable local value binding",
        NodeKind::CONSTANT => "constant fixed value configuration",
        NodeKind::ENUM_CONSTANT => "enum constant variant case",
        NodeKind::UNKNOWN => "unknown symbol unresolved placeholder",
    }
}

#[derive(Debug, Default)]
pub(crate) struct SemanticSymbolAliases {
    pub(crate) name_aliases: Vec<String>,
    pub(crate) terminal_alias: Option<String>,
    pub(crate) owner_aliases: Vec<String>,
}

pub(crate) fn semantic_symbol_aliases(
    display_name: &str,
    qualified_name: Option<&str>,
) -> SemanticSymbolAliases {
    let mut aliases = SemanticSymbolAliases::default();
    let mut seen_names = HashSet::new();
    let mut seen_owners = HashSet::new();
    let candidates = [Some(display_name), qualified_name];

    for candidate in candidates.into_iter().flatten() {
        if let Some(alias) = normalized_symbol_alias(candidate) {
            push_unique_alias(&mut aliases.name_aliases, &mut seen_names, alias);
        }
        if let Some(terminal) = terminal_symbol_part(candidate)
            && let Some(alias) = normalized_symbol_alias(terminal)
        {
            if aliases.terminal_alias.is_none() {
                aliases.terminal_alias = Some(alias.clone());
            }
            push_unique_alias(&mut aliases.name_aliases, &mut seen_names, alias);
        }

        let owner_parts = owner_symbol_parts(candidate);
        if let Some(owner) = owner_parts.last() {
            push_unique_alias(
                &mut aliases.owner_aliases,
                &mut seen_owners,
                (*owner).to_string(),
            );
            if let Some(alias) = normalized_symbol_alias(owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, alias);
            }
            for expanded_alias in expanded_symbol_aliases(owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, expanded_alias);
            }
        }
        if owner_parts.len() > 1 {
            let full_owner = owner_parts.join(" ");
            if let Some(alias) = normalized_symbol_alias(&full_owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, alias);
            }
            for expanded_alias in expanded_symbol_aliases(&full_owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, expanded_alias);
            }
        }
    }

    aliases
}

pub(crate) fn runtime_concept_phrases(
    display_name: &str,
    qualified_name: Option<&str>,
) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut seen = HashSet::new();
    let terminal_aliases = [Some(display_name), qualified_name]
        .into_iter()
        .flatten()
        .filter_map(terminal_symbol_part)
        .filter_map(normalized_symbol_alias)
        .collect::<Vec<_>>();

    for terminal in terminal_aliases {
        match terminal.as_str() {
            "sync llm symbol projection" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "synchronize persisted semantic documents after indexing".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "crates codestory runtime src lib sync semantic docs".to_string(),
                );
            }
            "grounding snapshot" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "make compact grounding overview with coverage buckets and notes".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "GroundingService grounding snapshot overview entrypoint returns compact notes and coverage buckets".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "grounding snapshot ranked file summaries coverage buckets".to_string(),
                );
            }
            "normalized hybrid weights" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "normalize lexical semantic and graph weights for retrieval".to_string(),
            ),
            "index file" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "index one source file with tree sitter symbols and semantic edges".to_string(),
            ),
            "reload llm docs from storage" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "reload persisted semantic documents from sqlite storage into the search index"
                    .to_string(),
            ),
            "build search hit output" | "search hit output" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "build JSON output object for a CLI search hit result".to_string(),
            ),
            "refresh plan" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "workspace refresh plan struct for added updated removed source files".to_string(),
            ),
            "node kind" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "rust language node kind enum for symbol graph node categories".to_string(),
            ),
            "extract members" | "fold edges" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "canonical member layout extracts members and folds symbol edges".to_string(),
            ),
            "compare resolution hits" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "resolution rank compares final search hits after candidate scoring".to_string(),
            ),
            "resolve profile" | "route auto preset" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "choose automatic retrieval preset from natural language agent prompt"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "agent retrieval profile selection routes auto preset policy and trail plans"
                        .to_string(),
                );
            }
            "to citation" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "turn scored retrieval hits into answer citations with target paths and line numbers"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "map hybrid search result score to AgentCitation node file path line evidence"
                        .to_string(),
                );
            }
            "resolution candidate rank" | "compare resolution candidates" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "rank candidate target matches by exactness kind and declaration anchors"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "compare resolved target candidates with file filter exact name kind declaration priority"
                        .to_string(),
                );
            }
            "source files" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "workspace source files discovery entrypoint lists indexed source files"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "workspace source files apply language filters and excludes".to_string(),
                );
            }
            "compile exclude patterns" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "compile glob exclude patterns for workspace source discovery".to_string(),
            ),
            "build llm symbol doc text" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "build embedded semantic search document text".to_string(),
            ),
            "trail context" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "follow outgoing graph edges around a focus symbol".to_string(),
            ),
            "build trail request" | "build trail request impl" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "build trail request from query focus node direction filters and max depth"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "trail request builder prepares graph traversal input for agent and CLI routes"
                        .to_string(),
                );
            }
            "graph trail" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "follow graph trail neighborhood edges for a focus symbol".to_string(),
            ),
            "handle stdio tool call" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "stdio tool router dispatches search symbol trail snippet and ask commands"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "dispatch JSON stdio tool calls to search symbol trail snippet and ask handlers"
                        .to_string(),
                );
            }
            "handle http request" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "http server router handles search symbol trail snippet ask requests and writes json"
                    .to_string(),
            ),
            "get resolution support snapshot" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "resolution support cache snapshot for import and call resolution".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "reuse or rebuild cached support tables used by semantic call and import resolution"
                        .to_string(),
                );
            }
            "prepare"
                if qualified_name
                    .is_some_and(|name| name.contains("ResolutionSupport::prepare")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "reuse or rebuild cached support tables used by semantic call and import resolution"
                        .to_string(),
                );
            }
            "semantic request key" | "semantic request target name" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "call resolution semantic request key target name candidate lookup".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "derive semantic resolution request keys and target names from unresolved edges"
                        .to_string(),
                );
            }
            "import name candidates" | "module prefix" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "import resolution candidates module prefix package name matching".to_string(),
            ),
            "no query match error" | "ambiguous query error" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "resolution ambiguity error for no query match or ambiguous query target"
                    .to_string(),
            ),
            "open project with storage path" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "camel case openProjectWithStoragePath method for opening sqlite storage"
                    .to_string(),
            ),
            "from env"
                if qualified_name.is_some_and(|name| {
                    name.contains("EmbeddingProfile::from_env")
                        || name.contains("EmbeddingBackendSelection::from_env")
                }) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "embedding profile environment variable backend selection from env"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "load embedding model profile prefixes pooling dimensions and backend choices from environment"
                        .to_string(),
                );
            }
            "llamacpp embeddings url env" | "parse llamacpp endpoint" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "llama.cpp OpenAI-compatible embeddings endpoint URL configuration".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "CODESTORY_EMBED_LLAMACPP_URL selects local HTTP embedding endpoint"
                        .to_string(),
                );
            }
            "resolve target" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "resolve user query to symbol id used by trail and snippet".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "resolve a user query to the selected SearchHit NodeId target for trail and snippet commands".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "map TargetSelection query or id into ResolvedTarget selected alternatives"
                        .to_string(),
                );
            }
            "node details" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "node details source occurrence edge digest for a symbol".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "node details entrypoint returns symbol metadata source and edge digest"
                        .to_string(),
                );
            }
            "semantic symbol aliases" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "semantic symbol aliases split namespaces camel snake acronyms".to_string(),
            ),
            "build config" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "Payload buildConfig root config registers collections content blocks dashboard widgets"
                    .to_string(),
            ),
            "content blocks" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "Payload content block registry array of reusable rich text blocks".to_string(),
            ),
            "hero field" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "Payload shared hero field group builder reusable field configuration".to_string(),
            ),
            "content field" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "Payload shared content richText field builder reusable field configuration"
                    .to_string(),
            ),
            "ast visitor" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor records declarations annotations references scopes and source ranges"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor records declarations annotations references scopes and source ranges while visiting AST nodes"
                        .to_string(),
                );
            }
            "record annotation" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor records annotation references and source ranges".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor recordAnnotation handles annotation references while visiting AST nodes"
                        .to_string(),
                );
            }
            "record scope" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor records local scope ranges for indexed symbols".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java AST visitor recordScope stores local scope ranges while visiting AST nodes"
                        .to_string(),
                );
            }
            "decl name resolver" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "Java declaration name resolver builds qualified declaration names for fields methods types enum constants and anonymous classes"
                    .to_string(),
            ),
            "get qualified decl name" | "get decl name" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "resolve Java AST declaration names into qualified DeclName values".to_string(),
            ),
            "function decl name" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java function declaration name stores return type parameter type signatures and static flag"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java function declaration names preserve parameter type signatures"
                        .to_string(),
                );
            }
            "variable decl name" => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java variable declaration name stores variable type and static flag".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "Java variable declaration names include static flags".to_string(),
                );
            }
            "java indexer" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "JavaIndexer Java class drives ASTParser parsing and records indexed symbols"
                    .to_string(),
            ),
            "process file"
                if qualified_name
                    .is_some_and(|name| name.contains("JavaIndexer.processFile")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "JavaIndexer processFile drives Java AST parsing for one source file"
                        .to_string(),
                );
            }
            "do record symbol"
                if qualified_name.is_some_and(|name| name.contains("JavaParser::doRecordSymbol")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "C++ JavaParser native bridge records symbols from Java indexer callbacks"
                        .to_string(),
                );
            }
            "get name hierarchy"
                if qualified_name.is_some_and(|name| name.contains("Node::getNameHierarchy")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph node returns type name hierarchy for node model traversal".to_string(),
                );
            }
            "find edge of type"
                if qualified_name.is_some_and(|name| name.contains("Node::findEdgeOfType")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph node finds parent child edge by type in node model".to_string(),
                );
            }
            "for each child node"
                if qualified_name.is_some_and(|name| name.contains("Node::forEachChildNode")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph node child traversal iterates each child node".to_string(),
                );
            }
            "get indexer command provider"
                if qualified_name
                    .is_some_and(|name| name.contains("SourceGroupCxxCdb::getIndexerCommandProvider")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "C++ compilation database source group chooses compile_commands json indexer command provider"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "choose compilation database source group when compile_commands json is used not Codeblocks project source group"
                        .to_string(),
                );
            }
            "compilation database" => push_unique_alias(
                &mut phrases,
                &mut seen,
                "compile_commands json compilation database for C++ source group indexing"
                    .to_string(),
            ),
            "create graph component"
                if qualified_name
                    .is_some_and(|name| name.contains("ComponentFactory::createGraphComponent")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "component factory creates graph controller component with graph view"
                        .to_string(),
                );
            }
            "create graph view"
                if qualified_name
                    .is_some_and(|name| name.contains("ViewFactory::createGraphView")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "view factory creates graph view instance without controller component"
                        .to_string(),
                );
            }
            "set active"
                if qualified_name.is_some_and(|name| name.contains("GraphController::setActive")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph controller setActive changes active node state not expanded nodes"
                        .to_string(),
                );
            }
            "set visibility"
                if qualified_name
                    .is_some_and(|name| name.contains("GraphController::setVisibility")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph controller setVisibility changes node visibility state not expanded nodes"
                        .to_string(),
                );
            }
            "get expanded node ids"
                if qualified_name
                    .is_some_and(|name| name.contains("GraphController::getExpandedNodeIds")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph controller returns expanded node id persistence state".to_string(),
                );
            }
            "load" if qualified_name.is_some_and(|name| name.contains("Project::load")) => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project load reads project settings and source groups before refresh and index build"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project loads settings refreshes source groups computes refresh info and builds an index"
                        .to_string(),
                );
            }
            "refresh" if qualified_name.is_some_and(|name| name.contains("Project::refresh")) => {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project refresh computes refresh info for source groups and source files"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project load refresh build index workflow refreshes source groups before indexing"
                        .to_string(),
                );
            }
            "build index"
                if qualified_name.is_some_and(|name| name.contains("Project::buildIndex")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project buildIndex builds the source graph index after load and refresh"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "project loads settings refreshes source groups computes refresh info and builds an index"
                        .to_string(),
                );
            }
            "add node as plain copy"
                if qualified_name.is_some_and(|name| name.contains("Graph::addNodeAsPlainCopy")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph plain copy operation duplicates existing node instead of creating a new node"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "plain-copy graph operations duplicate existing nodes while createNode allocates new graph elements"
                        .to_string(),
                );
            }
            "add edge as plain copy"
                if qualified_name.is_some_and(|name| name.contains("Graph::addEdgeAsPlainCopy")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "graph plain copy operation duplicates existing edge instead of creating a new edge"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "plain-copy graph operations duplicate existing edges while createEdge allocates new graph elements"
                        .to_string(),
                );
            }
            "get full text search locations"
                if qualified_name
                    .is_some_and(|name| name.contains("StorageAccess::getFullTextSearchLocations")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "storage access interface returns full text search source locations"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "full text search locations differ from autocompletion search matches for token activation"
                        .to_string(),
                );
            }
            "get autocompletion matches"
                if qualified_name
                    .is_some_and(|name| name.contains("StorageAccess::getAutocompletionMatches")) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "storage access interface returns autocompletion search matches"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "full text search locations differ from autocompletion search matches for token activation"
                        .to_string(),
                );
            }
            "parse"
                if qualified_name.is_some_and(|name| {
                    name.contains("TiXmlDocument::Parse") || name.contains("TiXmlElement::Parse")
                }) =>
            {
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "TinyXML document element parser parses XML source text".to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "TinyXML document parsing recognized as external XML parser code not project settings logic"
                        .to_string(),
                );
                push_unique_alias(
                    &mut phrases,
                    &mut seen,
                    "external TinyXML Parse method for document and element XML parser code"
                        .to_string(),
                );
                if qualified_name.is_some_and(|name| name.contains("TiXmlDocument::Parse")) {
                    push_unique_alias(
                        &mut phrases,
                        &mut seen,
                        "TiXmlDocument Parse parses a complete TinyXML document from XML source text"
                            .to_string(),
                    );
                }
                if qualified_name.is_some_and(|name| name.contains("TiXmlElement::Parse")) {
                    push_unique_alias(
                        &mut phrases,
                        &mut seen,
                        "TiXmlElement Parse parses TinyXML element nodes from XML source text"
                            .to_string(),
                    );
                }
            }
            _ => {}
        }
    }

    phrases
}

pub(crate) fn semantic_path_aliases(file_path: Option<&str>, limit: usize) -> Vec<String> {
    let Some(path) = file_path else {
        return Vec::new();
    };
    let extension = path.rsplit(['/', '\\']).next().and_then(|file_name| {
        file_name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    });
    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    for component in path
        .split(['/', '\\', '.'])
        .map(str::trim)
        .filter(|component| !component.is_empty())
    {
        if extension
            .as_deref()
            .is_some_and(|ext| component.eq_ignore_ascii_case(ext))
        {
            continue;
        }
        push_unique_alias(&mut aliases, &mut seen, component.to_string());
        if let Some(normalized) = normalized_symbol_alias(component)
            && normalized != component
        {
            push_unique_alias(&mut aliases, &mut seen, normalized);
        }
        if aliases.len() >= limit {
            aliases.truncate(limit);
            break;
        }
    }
    aliases
}

fn push_unique_alias(aliases: &mut Vec<String>, seen: &mut HashSet<String>, alias: String) {
    let alias = alias.trim();
    if alias.is_empty() || !seen.insert(alias.to_ascii_lowercase()) {
        return;
    }
    aliases.push(alias.to_string());
}

fn split_identifier_segment(segment: &str) -> Vec<String> {
    let chars = segment.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut current = String::new();

    for (idx, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                tokens.push(current.to_ascii_lowercase());
                current.clear();
            }
            continue;
        }

        let prev = idx.checked_sub(1).and_then(|prev| chars.get(prev)).copied();
        let next = chars.get(idx + 1).copied();
        let starts_new_token = !current.is_empty()
            && prev.is_some_and(|prev| {
                (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                    || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                    || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
                    || (prev.is_ascii_uppercase()
                        && ch.is_ascii_uppercase()
                        && next.is_some_and(|next| next.is_ascii_lowercase()))
            });
        if starts_new_token {
            tokens.push(current.to_ascii_lowercase());
            current.clear();
        }
        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current.to_ascii_lowercase());
    }

    tokens
}

fn symbol_alias_tokens(value: &str) -> Vec<String> {
    let normalized = value.replace("::", " ").replace("->", " ").replace(
        [
            '.', '#', '/', '\\', '_', '-', ':', '<', '>', '(', ')', '[', ']', '{', '}',
        ],
        " ",
    );
    normalized
        .split_whitespace()
        .flat_map(split_identifier_segment)
        .filter(|token| !token.is_empty())
        .collect()
}

fn normalized_symbol_alias(value: &str) -> Option<String> {
    let alias = symbol_alias_tokens(value).join(" ");
    (!alias.is_empty()).then_some(alias)
}

fn expanded_symbol_aliases(value: &str) -> Vec<String> {
    let tokens = symbol_alias_tokens(value);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut aliases = Vec::new();
    let mut seen = HashSet::new();

    if tokens.iter().any(|token| token == "cxx") {
        push_expanded_tokens(&tokens, &[("cxx", &["c++"][..])], &mut aliases, &mut seen);
        push_expanded_tokens(&tokens, &[("cxx", &["cpp"][..])], &mut aliases, &mut seen);
    }
    if tokens.iter().any(|token| token == "cdb") {
        let replacements = [
            ("cdb", &["compilation", "database"][..]),
            ("cdb", &["compile", "commands"][..]),
            ("cdb", &["compile", "commands", "json"][..]),
        ];
        for cdb_replacement in replacements {
            push_expanded_tokens(&tokens, &[cdb_replacement], &mut aliases, &mut seen);
            push_expanded_tokens(
                &tokens,
                &[("cxx", &["c++"][..]), cdb_replacement],
                &mut aliases,
                &mut seen,
            );
            push_expanded_tokens(
                &tokens,
                &[("cxx", &["cpp"][..]), cdb_replacement],
                &mut aliases,
                &mut seen,
            );
        }
    }

    aliases
}

fn push_expanded_tokens(
    tokens: &[String],
    replacements: &[(&str, &[&str])],
    aliases: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let expanded = tokens
        .iter()
        .flat_map(|token| {
            replacements
                .iter()
                .find_map(|(needle, replacement)| {
                    (token == needle).then(|| replacement.iter().copied())
                })
                .into_iter()
                .flatten()
                .chain(
                    (!replacements.iter().any(|(needle, _)| token == needle))
                        .then_some(token.as_str()),
                )
        })
        .collect::<Vec<_>>();
    push_unique_alias(aliases, seen, expanded.join(" "));
}

fn namespace_parts(value: &str) -> Vec<&str> {
    value
        .split("::")
        .flat_map(|part| part.split(['.', '#']))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn terminal_symbol_part(value: &str) -> Option<&str> {
    namespace_parts(value).into_iter().last()
}

fn owner_symbol_parts(value: &str) -> Vec<&str> {
    let mut parts = namespace_parts(value);
    if parts.len() <= 1 {
        return Vec::new();
    }
    parts.pop();
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_path_covers_supported_extensions() {
        let cases = [
            ("a.c", Some("c")),
            ("a.cc", Some("cpp")),
            ("a.cpp", Some("cpp")),
            ("a.cxx", Some("cpp")),
            ("a.h", Some("cpp")),
            ("a.hpp", Some("cpp")),
            ("A.JAVA", Some("java")),
            ("a.js", Some("javascript")),
            ("a.jsx", Some("javascript")),
            ("a.mjs", Some("javascript")),
            ("a.cjs", Some("javascript")),
            ("a.py", Some("python")),
            ("a.pyi", Some("python")),
            ("a.rs", Some("rust")),
            ("a.ts", Some("typescript")),
            ("a.tsx", Some("typescript")),
            ("a.mts", Some("typescript")),
            ("a.cts", Some("typescript")),
            ("README.md", None),
        ];

        for (path, language) in cases {
            assert_eq!(semantic_doc_language_from_path(Some(path)), language);
        }
    }

    #[test]
    fn symbol_aliases_split_namespaces_camel_snake_and_acronyms() {
        let aliases = semantic_symbol_aliases(
            "HTTPServer::parseURL2Value",
            Some("crate::net_io::HTTPServer::parseURL2Value"),
        );

        assert_eq!(
            aliases.name_aliases,
            vec![
                "http server parse url 2 value",
                "parse url 2 value",
                "crate net io http server parse url 2 value"
            ]
        );
        assert_eq!(aliases.terminal_alias.as_deref(), Some("parse url 2 value"));
        assert!(aliases.owner_aliases.contains(&"HTTPServer".to_string()));
        assert!(aliases.owner_aliases.contains(&"http server".to_string()));
    }

    #[test]
    fn symbol_aliases_expand_cpp_cdb_owner_acronyms() {
        let aliases = semantic_symbol_aliases(
            "SourceGroupCxxCdb::getIndexerCommandProvider",
            Some("sourcetrail::SourceGroupCxxCdb::getIndexerCommandProvider"),
        );

        assert!(
            aliases
                .owner_aliases
                .contains(&"source group c++ compilation database".to_string())
        );
        assert!(
            aliases
                .owner_aliases
                .contains(&"source group c++ compile commands json".to_string())
        );
    }

    #[test]
    fn runtime_concept_phrases_expand_targeted_runtime_terms_only() {
        assert_eq!(
            runtime_concept_phrases(
                "sync_llm_symbol_projection",
                Some("codestory_runtime::sync_llm_symbol_projection")
            ),
            vec![
                "synchronize persisted semantic documents after indexing",
                "crates codestory runtime src lib sync semantic docs"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "grounding_snapshot",
                Some("codestory_runtime::grounding::GroundingService::grounding_snapshot")
            ),
            vec![
                "make compact grounding overview with coverage buckets and notes",
                "GroundingService grounding snapshot overview entrypoint returns compact notes and coverage buckets",
                "grounding snapshot ranked file summaries coverage buckets"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "normalized_hybrid_weights",
                Some("codestory_runtime::support::normalized_hybrid_weights")
            ),
            vec!["normalize lexical semantic and graph weights for retrieval"]
        );
        assert_eq!(
            runtime_concept_phrases("index_file", Some("codestory_indexer::index_file")),
            vec!["index one source file with tree sitter symbols and semantic edges"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "reload_llm_docs_from_storage",
                Some("codestory_runtime::reload_llm_docs_from_storage")
            ),
            vec!["reload persisted semantic documents from sqlite storage into the search index"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "build_search_hit_output",
                Some("codestory_cli::build_search_hit_output")
            ),
            vec!["build JSON output object for a CLI search hit result"]
        );
        assert_eq!(
            runtime_concept_phrases("RefreshPlan", Some("codestory_workspace::RefreshPlan")),
            vec!["workspace refresh plan struct for added updated removed source files"]
        );
        assert_eq!(
            runtime_concept_phrases("NodeKind", Some("codestory_contracts::graph::NodeKind")),
            vec!["rust language node kind enum for symbol graph node categories"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "extract_members",
                Some("codestory_indexer::extract_members")
            ),
            vec!["canonical member layout extracts members and folds symbol edges"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "compare_resolution_hits",
                Some("codestory_runtime::compare_resolution_hits")
            ),
            vec!["resolution rank compares final search hits after candidate scoring"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "resolve_profile",
                Some("codestory_runtime::agent::profiles::resolve_profile")
            ),
            vec![
                "choose automatic retrieval preset from natural language agent prompt",
                "agent retrieval profile selection routes auto preset policy and trail plans"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "route_auto_preset",
                Some("codestory_runtime::agent::profiles::route_auto_preset")
            ),
            vec![
                "choose automatic retrieval preset from natural language agent prompt",
                "agent retrieval profile selection routes auto preset policy and trail plans"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "to_citation",
                Some("codestory_runtime::agent::orchestrator::to_citation")
            ),
            vec![
                "turn scored retrieval hits into answer citations with target paths and line numbers",
                "map hybrid search result score to AgentCitation node file path line evidence"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "resolution_candidate_rank",
                Some("codestory_cli::runtime::resolution_candidate_rank")
            ),
            vec![
                "rank candidate target matches by exactness kind and declaration anchors",
                "compare resolved target candidates with file filter exact name kind declaration priority"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "compare_resolution_candidates",
                Some("codestory_cli::runtime::compare_resolution_candidates")
            ),
            vec![
                "rank candidate target matches by exactness kind and declaration anchors",
                "compare resolved target candidates with file filter exact name kind declaration priority"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("source_files", Some("codestory_workspace::source_files")),
            vec![
                "workspace source files discovery entrypoint lists indexed source files",
                "workspace source files apply language filters and excludes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "compile_exclude_patterns",
                Some("codestory_workspace::compile_exclude_patterns")
            ),
            vec!["compile glob exclude patterns for workspace source discovery"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "build_llm_symbol_doc_text",
                Some("codestory_runtime::build_llm_symbol_doc_text")
            ),
            vec!["build embedded semantic search document text"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "open_project_with_storage_path",
                Some("codestory_runtime::ProjectService::open_project_with_storage_path")
            ),
            vec!["camel case openProjectWithStoragePath method for opening sqlite storage"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "handle_stdio_tool_call",
                Some("codestory_cli::stdio::handle_stdio_tool_call")
            ),
            vec![
                "stdio tool router dispatches search symbol trail snippet and ask commands",
                "dispatch JSON stdio tool calls to search symbol trail snippet and ask handlers"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "handle_http_request",
                Some("codestory_cli::serve::handle_http_request")
            ),
            vec![
                "http server router handles search symbol trail snippet ask requests and writes json"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "build_trail_request",
                Some("codestory_runtime::agent::orchestrator::build_trail_request")
            ),
            vec![
                "build trail request from query focus node direction filters and max depth",
                "trail request builder prepares graph traversal input for agent and CLI routes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "semantic_request_key",
                Some("codestory_indexer::semantic_request_key")
            ),
            vec![
                "call resolution semantic request key target name candidate lookup",
                "derive semantic resolution request keys and target names from unresolved edges"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "get_resolution_support_snapshot",
                Some("codestory_store::Storage::get_resolution_support_snapshot")
            ),
            vec![
                "resolution support cache snapshot for import and call resolution",
                "reuse or rebuild cached support tables used by semantic call and import resolution"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "prepare",
                Some("codestory_indexer::ResolutionSupport::prepare")
            ),
            vec![
                "reuse or rebuild cached support tables used by semantic call and import resolution"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "import_name_candidates",
                Some("codestory_indexer::import_name_candidates")
            ),
            vec!["import resolution candidates module prefix package name matching"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "no_query_match_error",
                Some("codestory_runtime::no_query_match_error")
            ),
            vec!["resolution ambiguity error for no query match or ambiguous query target"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "from_env",
                Some("codestory_runtime::search::engine::EmbeddingProfile::from_env")
            ),
            vec![
                "embedding profile environment variable backend selection from env",
                "load embedding model profile prefixes pooling dimensions and backend choices from environment"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "resolve_target",
                Some("codestory_cli::runtime::resolve_target")
            ),
            vec![
                "resolve user query to symbol id used by trail and snippet",
                "resolve a user query to the selected SearchHit NodeId target for trail and snippet commands",
                "map TargetSelection query or id into ResolvedTarget selected alternatives"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "LLAMACPP_EMBEDDINGS_URL_ENV",
                Some("codestory_runtime::search::engine::LLAMACPP_EMBEDDINGS_URL_ENV")
            ),
            vec![
                "llama.cpp OpenAI-compatible embeddings endpoint URL configuration",
                "CODESTORY_EMBED_LLAMACPP_URL selects local HTTP embedding endpoint"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "parse",
                Some("codestory_runtime::search::engine::LlamaCppEndpoint::parse")
            ),
            vec![
                "llama.cpp OpenAI-compatible embeddings endpoint URL configuration",
                "CODESTORY_EMBED_LLAMACPP_URL selects local HTTP embedding endpoint"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "semantic_doc_text_for_test",
                Some("tests::semantic_doc_text_for_test")
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            runtime_concept_phrases(
                "node_details",
                Some("codestory_runtime::grounding::GroundingService::node_details")
            ),
            vec![
                "node details source occurrence edge digest for a symbol",
                "node details entrypoint returns symbol metadata source and edge digest"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "semantic_symbol_aliases",
                Some("codestory_runtime::semantic_symbol_aliases")
            ),
            vec!["semantic symbol aliases split namespaces camel snake acronyms"]
        );
        assert_eq!(
            runtime_concept_phrases("buildConfig", Some("buildConfig")),
            vec![
                "Payload buildConfig root config registers collections content blocks dashboard widgets"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("contentBlocks", Some("contentBlocks")),
            vec!["Payload content block registry array of reusable rich text blocks"]
        );
        assert_eq!(
            runtime_concept_phrases("heroField", Some("heroField")),
            vec!["Payload shared hero field group builder reusable field configuration"]
        );
        assert_eq!(
            runtime_concept_phrases("contentField", Some("contentField")),
            vec!["Payload shared content richText field builder reusable field configuration"]
        );
        assert_eq!(
            runtime_concept_phrases("AstVisitor", Some("com.sourcetrail.AstVisitor")),
            vec![
                "Java AST visitor records declarations annotations references scopes and source ranges",
                "Java AST visitor records declarations annotations references scopes and source ranges while visiting AST nodes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "recordAnnotation",
                Some("com.sourcetrail.AstVisitor.recordAnnotation")
            ),
            vec![
                "Java AST visitor records annotation references and source ranges",
                "Java AST visitor recordAnnotation handles annotation references while visiting AST nodes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "recordScope",
                Some("com.sourcetrail.AstVisitor.recordScope")
            ),
            vec![
                "Java AST visitor records local scope ranges for indexed symbols",
                "Java AST visitor recordScope stores local scope ranges while visiting AST nodes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "DeclNameResolver",
                Some("com.sourcetrail.name.resolver.DeclNameResolver")
            ),
            vec![
                "Java declaration name resolver builds qualified declaration names for fields methods types enum constants and anonymous classes"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "getQualifiedDeclName",
                Some("com.sourcetrail.name.resolver.DeclNameResolver.getQualifiedDeclName")
            ),
            vec!["resolve Java AST declaration names into qualified DeclName values"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "FunctionDeclName",
                Some("com.sourcetrail.name.FunctionDeclName")
            ),
            vec![
                "Java function declaration name stores return type parameter type signatures and static flag",
                "Java function declaration names preserve parameter type signatures"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "VariableDeclName",
                Some("com.sourcetrail.name.VariableDeclName")
            ),
            vec![
                "Java variable declaration name stores variable type and static flag",
                "Java variable declaration names include static flags"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("JavaIndexer", Some("com.sourcetrail.JavaIndexer")),
            vec!["JavaIndexer Java class drives ASTParser parsing and records indexed symbols"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "processFile",
                Some("com.sourcetrail.JavaIndexer.processFile")
            ),
            vec!["JavaIndexer processFile drives Java AST parsing for one source file"]
        );
        assert_eq!(
            runtime_concept_phrases("processFile", Some("codestory_workspace::processFile")),
            Vec::<String>::new()
        );
        assert_eq!(
            runtime_concept_phrases("doRecordSymbol", Some("JavaParser::doRecordSymbol")),
            vec!["C++ JavaParser native bridge records symbols from Java indexer callbacks"]
        );
        assert_eq!(
            runtime_concept_phrases("getNameHierarchy", Some("Node::getNameHierarchy")),
            vec!["graph node returns type name hierarchy for node model traversal"]
        );
        assert_eq!(
            runtime_concept_phrases("findEdgeOfType", Some("Node::findEdgeOfType")),
            vec!["graph node finds parent child edge by type in node model"]
        );
        assert_eq!(
            runtime_concept_phrases("forEachChildNode", Some("Node::forEachChildNode")),
            vec!["graph node child traversal iterates each child node"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "getIndexerCommandProvider",
                Some("SourceGroupCxxCdb::getIndexerCommandProvider")
            ),
            vec![
                "C++ compilation database source group chooses compile_commands json indexer command provider",
                "choose compilation database source group when compile_commands json is used not Codeblocks project source group"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("CompilationDatabase", Some("CompilationDatabase")),
            vec!["compile_commands json compilation database for C++ source group indexing"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "createGraphComponent",
                Some("ComponentFactory::createGraphComponent")
            ),
            vec!["component factory creates graph controller component with graph view"]
        );
        assert_eq!(
            runtime_concept_phrases("createGraphView", Some("ViewFactory::createGraphView")),
            vec!["view factory creates graph view instance without controller component"]
        );
        assert_eq!(
            runtime_concept_phrases("setActive", Some("GraphController::setActive")),
            vec!["graph controller setActive changes active node state not expanded nodes"]
        );
        assert_eq!(
            runtime_concept_phrases("setVisibility", Some("GraphController::setVisibility")),
            vec!["graph controller setVisibility changes node visibility state not expanded nodes"]
        );
        assert_eq!(
            runtime_concept_phrases(
                "getExpandedNodeIds",
                Some("GraphController::getExpandedNodeIds")
            ),
            vec!["graph controller returns expanded node id persistence state"]
        );
        assert_eq!(
            runtime_concept_phrases("load", Some("Project::load")),
            vec![
                "project load reads project settings and source groups before refresh and index build",
                "project loads settings refreshes source groups computes refresh info and builds an index"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("refresh", Some("Project::refresh")),
            vec![
                "project refresh computes refresh info for source groups and source files",
                "project load refresh build index workflow refreshes source groups before indexing"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("buildIndex", Some("Project::buildIndex")),
            vec![
                "project buildIndex builds the source graph index after load and refresh",
                "project loads settings refreshes source groups computes refresh info and builds an index"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("addNodeAsPlainCopy", Some("Graph::addNodeAsPlainCopy")),
            vec![
                "graph plain copy operation duplicates existing node instead of creating a new node",
                "plain-copy graph operations duplicate existing nodes while createNode allocates new graph elements"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("addEdgeAsPlainCopy", Some("Graph::addEdgeAsPlainCopy")),
            vec![
                "graph plain copy operation duplicates existing edge instead of creating a new edge",
                "plain-copy graph operations duplicate existing edges while createEdge allocates new graph elements"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "getFullTextSearchLocations",
                Some("StorageAccess::getFullTextSearchLocations")
            ),
            vec![
                "storage access interface returns full text search source locations",
                "full text search locations differ from autocompletion search matches for token activation"
            ]
        );
        assert_eq!(
            runtime_concept_phrases(
                "getAutocompletionMatches",
                Some("StorageAccess::getAutocompletionMatches")
            ),
            vec![
                "storage access interface returns autocompletion search matches",
                "full text search locations differ from autocompletion search matches for token activation"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("Parse", Some("TiXmlDocument::Parse")),
            vec![
                "TinyXML document element parser parses XML source text",
                "TinyXML document parsing recognized as external XML parser code not project settings logic",
                "external TinyXML Parse method for document and element XML parser code",
                "TiXmlDocument Parse parses a complete TinyXML document from XML source text"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("Parse", Some("TiXmlElement::Parse")),
            vec![
                "TinyXML document element parser parses XML source text",
                "TinyXML document parsing recognized as external XML parser code not project settings logic",
                "external TinyXML Parse method for document and element XML parser code",
                "TiXmlElement Parse parses TinyXML element nodes from XML source text"
            ]
        );
        assert_eq!(
            runtime_concept_phrases("Parse", Some("Project::Parse")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn path_aliases_keep_raw_and_normalized_components_without_extension() {
        assert_eq!(
            semantic_path_aliases(Some("crates/codestory-runtime/src/lib.rs"), 8),
            vec![
                "crates",
                "codestory-runtime",
                "codestory runtime",
                "src",
                "lib"
            ]
        );
    }
}
