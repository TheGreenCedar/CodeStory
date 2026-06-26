# Documentation Maintenance Checklist

**Audience:** Contributors

Guidelines for keeping user docs task-first and contributor or agent docs
contract-complete.

## Documentation Lanes

| Lane | Primary home | Owner | Writing bar |
| --- | --- | --- | --- |
| User / operator | `docs/users/`, root `README.md` | User docs lane | Start from the human task; link to [glossary](../glossary.md) for terms; avoid subsystem internals unless needed for repair |
| Trust / readiness | [users/trust-and-readiness.md](../users/trust-and-readiness.md) | Trust lane | Plain-language readiness boundaries; link agent field detail to skill `status-contract.md`; no status-field tables on user hub |
| Prompt patterns | [users/prompt-patterns.md](../users/prompt-patterns.md) | Prompts lane | Portable good/bad examples; no host-specific ritual duplication |
| CLI reference | [users/cli-reference.md](../users/cli-reference.md) | CLI lane | Power-user repair only; opening must defer first install to user hub + trust/troubleshooting |
| Contributor | `docs/contributors/`, `AGENTS.md` | Contributor lane | CLI-centric workflows, verification lanes, and crate ownership are fine |
| Architecture | `docs/architecture/` | Architecture lane | Structure, diagrams, and subsystem boundaries; no install quick starts |
| Evidence / benchmarks | `docs/testing/` | Evidence lane | Evidence records and promotion gates; not install guides |
| Operations | `docs/ops/` | Ops lane | Runbooks and maintainer internals; link user repair paths to `docs/users/troubleshooting.md` |

## Structure & Organization

### Content completeness
- [ ] Every major feature has dedicated documentation in the correct lane
- [ ] Trust boundaries are clearly defined and documented
- [ ] Examples cover the major workflows for that lane
- [ ] Error conditions and edge cases are documented
- [ ] Configuration options are fully documented where operators or contributors need them

### Example quality
- [ ] User examples use the reader's project symbols and paths, not CodeStory-internal crate names unless the page is about this repository
- [ ] Contributor examples may use this workspace when proving a change locally
- [ ] Examples are adaptable to different repositories
- [ ] Examples include expected output and behavior when behavior is non-obvious
- [ ] Examples demonstrate both success and error cases when trust boundaries matter

### Cross-reference quality
- [ ] Internal references use correct relative paths
- [ ] User journeys link to `docs/users/` instead of contributor or architecture pages
- [ ] One canonical owner per topic — no redirect stubs or duplicate operator hubs
- [ ] Contributor pages link to `docs/contributors/` and `docs/testing/` for proof lanes
- [ ] References to commands, files, and concepts are up-to-date
- [ ] Navigation between related documents is clear
- [ ] Trust boundary documentation is consistent across lanes

### Maintenance guidelines
- [ ] Documentation follows the repository's coding style
- [ ] All documentation files have proper headers and structure
- [ ] Code snippets are properly formatted and syntax-highlighted
- [ ] Markdown links are validated and working
- [ ] Documentation is kept in sync with code changes

## Content Review Process

### Documentation lanes
- [ ] **Docs-only changes**: Verify with `git diff --check` and `node .github/scripts/check-doc-links.mjs`
- [ ] **Do not add doc prose unit tests** — structure gates only (link CI, `git diff --check`, plugin-static for plugin/runtime shape)
- [ ] **No redirect stubs** — one canonical owner per topic under `docs/users/` or the listed lane home
- [ ] **CLI changes**: Run `cargo test -p codestory-cli`
- [ ] **Runtime changes**: Run `cargo test -p codestory-runtime`
- [ ] **Indexer changes**: Run full indexer fidelity suites
- [ ] **Store changes**: Run `cargo test -p codestory-store`
- [ ] **Release changes**: Run release scripts in testing matrix

### Verification process
- [ ] Run `cargo fmt --check` on all documentation-related code
- [ ] Run `cargo check` to ensure no documentation compilation errors
- [ ] Run `cargo clippy --all-targets -- -D warnings` for linting
- [ ] Run plugin static tests with `node --test plugins/codestory/tests/plugin-static.test.mjs` when plugin files change (structure and runtime shape only — not doc copy)
- [ ] Check for broken links with `node .github/scripts/check-doc-links.mjs` (covers `README.md`, `docs/**` including templates, `plugins/codestory/README.md`, `plugins/codestory/docs/**`, and `plugins/codestory/skills/**`)

## Documentation Structure

### Current structure
- **README.md**: Concise overview and quick start
- **docs/README.md**: Routing document across lanes
- **docs/users/**: Host guides, troubleshooting, and CLI reference for operators
- **docs/architecture/**: System architecture and design
- **docs/glossary.md**: Core concepts and terminology
- **docs/contributors/**: Contributor guidelines and setup
- **docs/testing/**: Testing procedures, benchmarks, and evidence logs
- **docs/ops/**: Operational procedures and maintenance

### Documentation flow
- [ ] User docs start from the job the operator needs to do
- [ ] Contributor docs start from crate ownership or verification lane
- [ ] Architecture docs start from structure, diagrams, or execution paths
- [ ] Follow the trust boundary guidance for the lane
- [ ] Use the verification lane picker for code changes

### Documentation templates
- [ ] Use the [documentation template](../templates/documentation-template.md) for new files
- [ ] Use the [README template](../templates/readme-template.md) for main README files
- [ ] Use the [operator journey template](../templates/operator-journey-template.md) for user workflow documentation
- [ ] Use the [contributor setup template](../templates/contributor-setup-template.md) for contributor guidance

### Template usage guidelines

**When to use each template:**

- **Documentation template**: For any new documentation file that doesn't fit into existing categories
- **README template**: For main project README files that provide overview and quick start
- **Operator journey template**: For user workflow documentation in `docs/users/`
- **Contributor setup template**: For documentation that guides contributors through development and verification

**Template maintenance:**

- Keep templates up-to-date with current documentation patterns
- Review templates periodically for improvements
- Update templates when documentation structure changes
- Ensure templates reflect current best practices

**Template customization:**

- Adapt templates to the target lane
- User templates should not require CodeStory-repo dogfood examples
- Contributor templates may reference this workspace when proving local changes
- Maintain consistency across all documentation files

## Best Practices

### Writing guidelines
- [ ] Use active voice and imperative tone
- [ ] Keep sentences short and focused
- [ ] Use tables for comparisons and options
- [ ] Use code blocks for commands and examples
- [ ] Use proper markdown formatting
- [ ] Do not open architecture or contributor pages with product thesis statements

### Example guidelines
- [ ] User examples cite the reader's paths and symbols
- [ ] Contributor examples may cite `crates/*` paths when debugging this repo
- [ ] Include expected output and behavior when behavior is non-obvious
- [ ] Show the complete command or workflow
- [ ] Include relevant flags and options

### Maintenance guidelines
- [ ] Update documentation when code changes
- [ ] Keep examples up-to-date with current behavior
- [ ] Fix broken links and references
- [ ] Review documentation for clarity and completeness
- [ ] Run plugin static tests before committing when plugin files change (validates adapter/skill structure and runtime wiring — not documentation prose)

## Documentation Quality Gates

### Before committing
- [ ] Run `git diff --check` to ensure no whitespace issues (required doc gate)
- [ ] Run plugin static tests with `node --test plugins/codestory/tests/plugin-static.test.mjs` when plugin files change (structure/runtime only)
- [ ] `node .github/scripts/check-doc-links.mjs` passes (structure gate — relative paths and anchors; not prose phrase tests)
- [ ] Check for any documentation compilation errors
- [ ] Ensure all examples are syntactically correct
- [ ] Verify all internal references are working
- [ ] Check for consistent formatting and structure
- [ ] Ensure user docs remain user-first and contributor or agent docs remain contract-complete
- [ ] Ensure documentation follows the appropriate template
- [ ] Verify that documentation examples are testable where they claim behavior
- [ ] Check for proper markdown syntax and formatting

### Before merging
- [ ] Run full documentation verification suite for the touched lane
- [ ] Review documentation for completeness and accuracy
- [ ] Ensure user examples are portable across repositories
- [ ] Check for any broken links or references
- [ ] Validate documentation structure and organization
- [ ] Verify that all key concepts are explained or linked from the glossary
- [ ] Check for consistent terminology
- [ ] Ensure documentation meets the trust boundary requirements
- [ ] Validate that documentation follows the project's coding style
- [ ] Check for any documentation linting issues
- [ ] Ensure documentation is up-to-date with current code behavior
- [ ] Verify that examples work with the current codebase when they claim runtime behavior

## Documentation Tools

### Available tools
- [ ] `git diff --check`: Required whitespace gate for docs-only changes
- [ ] `node .github/scripts/check-doc-links.mjs`: Validates relative links in `README.md`, `docs/**` (including templates), `plugins/codestory/README.md`, `plugins/codestory/docs/**`, and `plugins/codestory/skills/**` — no prose phrase tests
- [ ] `cargo fmt --check`: Ensures Rust code style consistency
- [ ] `cargo check`: Catches documentation compilation errors
- [ ] `cargo clippy`: Identifies documentation lint issues
- [ ] `node --test plugins/codestory/tests/plugin-static.test.mjs`: Validates plugin adapter/skill structure and runtime wiring — not documentation copy

### Automation
- [ ] CI/CD pipeline runs documentation verification (`git diff --check`, `docs link check` workflow)
- [ ] Automated checks for broken links and references (structure only — no doc prose unit tests)
- [ ] Documentation linting and formatting checks
- [ ] Version control hooks for documentation changes

## Ongoing Maintenance

### Regular tasks
- [ ] Review documentation for outdated information
- [ ] Update examples to reflect current behavior
- [ ] Fix any broken links or references
- [ ] Add documentation for new features in the correct lane
- [ ] Review and improve documentation quality

### Periodic reviews
- [ ] Quarterly documentation audit
- [ ] Annual documentation structure review
- [ ] Documentation quality assessment
- [ ] User feedback collection and analysis
- [ ] Documentation roadmap planning
