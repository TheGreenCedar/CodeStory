# Documentation Maintenance Checklist

This checklist provides guidelines for maintaining high-quality, consistent documentation across the CodeStory repository.

## Structure & Organization

### ✅ Content Completeness
- [ ] Every major feature has dedicated documentation
- [ ] Trust boundaries are clearly defined and documented
- [ ] Examples are provided for all major workflows
- [ ] Error conditions and edge cases are documented
- [ ] Configuration options are fully documented

### ✅ Example Quality
- [ ] Examples use concrete repository terms
- [ ] Examples are adaptable to different repositories
- [ ] Examples include expected output and behavior
- [ ] Examples demonstrate both success and error cases
- [ ] Examples avoid generic architecture words

### ✅ Cross-Reference Quality
- [ ] Internal references use correct relative paths
- [ ] External references use proper markdown link format
- [ ] References to commands, files, and concepts are up-to-date
- [ ] Navigation between related documents is clear
- [ ] Trust boundary documentation is consistent

### ✅ Maintenance Guidelines
- [ ] Documentation follows the repository's coding style
- [ ] All documentation files have proper headers and structure
- [ ] Code snippets are properly formatted and syntax-highlighted
- [ ] Markdown links are validated and working
- [ ] Documentation is kept in sync with code changes

## Content Review Process

### ✅ Documentation Lanes
- [ ] **Docs-only changes**: Verify with `git diff --check`
- [ ] **CLI changes**: Run `cargo test -p codestory-cli`
- [ ] **Runtime changes**: Run `cargo test -p codestory-runtime`
- [ ] **Indexer changes**: Run full indexer fidelity suites
- [ ] **Store changes**: Run `cargo test -p codestory-store`
- [ ] **Release changes**: Run release scripts in testing matrix

### ✅ Verification Process
- [ ] Run `cargo fmt --check` on all documentation-related code
- [ ] Run `cargo check` to ensure no documentation compilation errors
- [ ] Run `cargo clippy --all-targets -- -D warnings` for linting
- [ ] Validate plugin documentation with `node --test plugins/codestory/tests/plugin-static.test.mjs`
- [ ] Check for broken links and references

## Documentation Structure

### ✅ Current Structure
- **README.md**: Concise overview and quick start
- **docs/README.md**: True routing document
- **docs/usage.md**: Operator journey and workflows
- **docs/architecture/**: System architecture and design
- **docs/concepts/**: Core concepts and terminology
- **docs/contributors/**: Contributor guidelines and setup
- **docs/testing/**: Testing procedures and benchmarks
- **docs/ops/**: Operational procedures and maintenance

### ✅ Documentation Flow
- [ ] Start from the job you need to do
- [ ] Use concrete examples specific to your repository
- [ ] Adapt examples to your project's structure and terminology
- [ ] Follow the trust boundary guidance
- [ ] Use the verification lane picker for changes

### ✅ Documentation Templates
- [ ] Use the [documentation template](../templates/documentation-template.md) for new files
- [ ] Use the [README template](../templates/readme-template.md) for main README files
- [ ] Use the [operator journey template](../templates/operator-journey-template.md) for journey documentation
- [ ] Use the [contributor setup template](../templates/contributor-setup-template.md) for contributor guidance

### ✅ Template Usage Guidelines

**When to use each template:**

- **Documentation template**: For any new documentation file that doesn't fit into existing categories
- **README template**: For main project README files that provide overview and quick start
- **Operator journey template**: For documentation that guides users through workflows and operations
- **Contributor setup template**: For documentation that guides contributors through development and verification

**Template maintenance:**

- Keep templates up-to-date with current documentation patterns
- Review templates periodically for improvements
- Update templates when documentation structure changes
- Ensure templates reflect current best practices

**Template customization:**

- Adapt templates to specific project needs
- Include project-specific examples and terminology
- Follow existing documentation patterns when using templates
- Maintain consistency across all documentation files

## Best Practices

### ✅ Writing Guidelines
- [ ] Use active voice and imperative tone
- [ ] Keep sentences short and focused
- [ ] Use tables for comparisons and options
- [ ] Use code blocks for commands and examples
- [ ] Use proper markdown formatting

### ✅ Example Guidelines
- [ ] Use concrete file paths and symbols
- [ ] Include expected output and behavior
- [ ] Demonstrate both success and error cases
- [ ] Show the complete command or workflow
- [ ] Include relevant flags and options

### ✅ Maintenance Guidelines
- [ ] Update documentation when code changes
- [ ] Keep examples up-to-date with current behavior
- [ ] Fix broken links and references
- [ ] Review documentation for clarity and completeness
- [ ] Run documentation verification tests before committing

## Documentation Quality Gates

### ✅ Before Committing
- [ ] Run `git diff --check` to ensure no whitespace issues
- [ ] Validate plugin documentation with `node --test plugins/codestory/tests/plugin-static.test.mjs`
- [ ] Check for any documentation compilation errors
- [ ] Ensure all examples are syntactically correct
- [ ] Verify all internal references are working
- [ ] Check for consistent formatting and structure
- [ ] Validate that examples follow the adaptation guidance
- [ ] Ensure documentation follows the appropriate template
- [ ] Verify that documentation examples are testable
- [ ] Check for proper markdown syntax and formatting

### ✅ Before Merging
- [ ] Run full documentation verification suite
- [ ] Review documentation for completeness and accuracy
- [ ] Ensure examples are generalizable and adaptable
- [ ] Check for any broken links or references
- [ ] Validate documentation structure and organization
- [ ] Verify that all key concepts are explained
- [ ] Check for consistent terminology
- [ ] Ensure documentation meets the trust boundary requirements
- [ ] Validate that documentation follows the project's coding style
- [ ] Check for any documentation linting issues
- [ ] Ensure documentation is up-to-date with current code behavior
- [ ] Verify that all examples work with the current codebase

## Documentation Tools

### ✅ Available Tools
- [ ] `git diff --check`: Validates documentation formatting
- [ ] `cargo fmt --check`: Ensures Rust code style consistency
- [ ] `cargo check`: Catches documentation compilation errors
- [ ] `cargo clippy`: Identifies documentation lint issues
- [ ] `node --test plugins/codestory/tests/plugin-static.test.mjs`: Validates plugin documentation

### ✅ Automation
- [ ] CI/CD pipeline runs documentation verification
- [ ] Automated checks for broken links and references
- [ ] Documentation linting and formatting checks
- [ ] Version control hooks for documentation changes

## Ongoing Maintenance

### ✅ Regular Tasks
- [ ] Review documentation for outdated information
- [ ] Update examples to reflect current behavior
- [ ] Fix any broken links or references
- [ ] Add documentation for new features
- [ ] Review and improve documentation quality

### ✅ Periodic Reviews
- [ ] Quarterly documentation audit
- [ ] Annual documentation structure review
- [ ] Documentation quality assessment
- [ ] User feedback collection and analysis
- [ ] Documentation roadmap planning

This checklist provides a comprehensive framework for maintaining high-quality, consistent documentation across the CodeStory repository. Regular adherence to these guidelines ensures that documentation remains accurate, useful, and maintainable.