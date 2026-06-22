# Documentation Search Index

Keyword index for CodeStory documentation. For routing by job, start at
[docs/README.md](README.md). Canonical owners are listed there.

## Documentation Sections

### Main Documentation

#### README.md
- **Quick start**: Plugin installation and initial setup
- **Example prompts**: CodeStory-repo examples with link to portable templates
- **What your agent gets**: Capabilities and readiness lanes
- **CLI escape hatch**: Command-line interface usage
- **With vs without CodeStory**: Focused and holdout benchmark comparison
- **Documentation**: Navigation to other documentation

#### docs/README.md
- **First stop**: Reader job to canonical doc mapping
- **Common paths**: Question-to-doc routing
- **Evidence surfaces**: Trust boundary documentation
- **Canonical owners**: Which doc owns operator flow, terms, verification
- **Documentation maintenance**: Search index, checklist, templates

#### docs/usage.md
- **Operator Journey**: Step-by-step user journey
- **Example prompts**: Portable templates and CodeStory-repo examples
- **Readiness Lanes**: Local navigation vs agent packet/search
- **Local Navigation**: Commands for cache-local exact-target source inspection
- **Broad Packet/Search**: Commands for sidecar-backed discovery
- **Stale Local Cache**: Refresh procedures
- **Sidecar Repair**: Sidecar setup and repair procedures
- **Output And Configuration**: Command output and configuration options
- **Command Cheat Sheet**: Quick reference for all commands
- **Verification**: Documentation verification procedures

#### docs/architecture/
- **overview.md**: Architecture overview and system layers
- **runtime-execution-path.md**: Runtime execution path documentation
- **indexing-pipeline.md**: Indexing pipeline documentation
- **retrieval-design.md**: Retrieval design and architecture
- **language-support.md**: Language support claims and coverage
- **subsystems/**: Subsystem-specific documentation

#### docs/concepts/
- **how-codestory-works.md**: Core concepts and functionality

#### docs/contributors/
- **getting-started.md**: Contributor setup and verification
- **testing-matrix.md**: Testing matrix and verification lanes
- **debugging.md**: Debugging guide
- **documentation-maintenance-checklist.md**: Documentation quality gates

#### docs/templates/
- **documentation-template.md**: General documentation structure
- **readme-template.md**: README structure
- **operator-journey-template.md**: Operator journey structure
- **contributor-setup-template.md**: Contributor setup structure

#### docs/testing/
- **agent-benchmark-harness-verification.md**: Benchmark harness verification
- **language-expansion-holdout-stats.md**: Language expansion holdout statistics
- **codestory-e2e-stats-log.md**: E2E stats log documentation
- **performance-review-playbook.md**: Performance review playbook
- **embedding-backend-benchmarks.md**: Embedding backend benchmarks
- **retrieval-architecture.md**: Sidecar promotion gates and proof tiers
- **codestory-stdio-warm-loop-stats.md**: Stdin/stdout warm loop statistics

#### docs/ops/
- **retrieval-sidecars.md**: Retrieval sidecars operations

#### Other
- **glossary.md**: Terminology across operator, architecture, and verification docs
- **research.md**: Research handbook for retrieval and embedding decisions

## Key Concepts Index

See [glossary.md](glossary.md) for canonical definitions. Summary:

### Readiness Concepts
- **Local navigation**: SQLite cache, graph, and DB-backed browse commands
- **Agent packet/search**: Sidecars healthy and `retrieval_mode=full`
- **Retrieval mode**: Sidecar status contract; only `full` serves agent packet/search
- **Semantic ready**: Dense-anchor embedding state matches policy

### System Concepts
- **Runtime**: Orchestrates indexing, grounding, trails, packet/search flows
- **Workspace**: Manifest and discovery layer for project files
- **Contracts**: Shared graph types, DTOs, and events across crates
- **Target context**: DB-first bundle for one concrete target
- **Cache root**: Directory for one project cache

## Example Prompt Templates

Portable shapes (any repository):

```text
@CodeStory check local_navigation and agent_packet_search on this checkout, ground the repo, and tell me whether sidecars need repair before I use packet.
```

```text
@CodeStory Where is [TARGET_FEATURE] defined and who calls it?
```

```text
@CodeStory I am editing [PATH_TO_FILE]. What symbols are affected and what tests should I run first?
```

CodeStory-repo dogfood examples: [usage.md - Example prompts](usage.md#example-prompts).

## Navigation Paths

### For First-Time Users
1. Start with [README.md - Quick start](../README.md#quick-start)
2. Read [Usage - Operator Journey](usage.md#operator-journey)
3. Use example prompts to understand the workflow

### For Contributors
1. Start with [Contributor setup](contributors/getting-started.md)
2. Use the [verification lane picker](contributors/getting-started.md#choose-the-verification-lane-first)
3. Follow the recommended reading order for building mental models

### For Reviewers
1. Start with [Testing matrix](contributors/testing-matrix.md)
2. Use the verification lane picker to determine the appropriate testing approach
3. Review [documentation maintenance checklist](contributors/documentation-maintenance-checklist.md) for docs-only changes

### For Researchers
1. Start with [Research handbook](research.md)
2. Use the comparison matrix for embedding and retrieval experiments
3. Review timing and benchmark records

## Maintenance

Update this index when adding major doc pages. Canonical command and verification
details live in [usage.md](usage.md) and
[contributors/testing-matrix.md](contributors/testing-matrix.md).
