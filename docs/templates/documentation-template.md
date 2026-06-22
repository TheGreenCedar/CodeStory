# Documentation Template

## Overview

This template provides a consistent structure for CodeStory documentation files.

## Required Sections

### Header
- File title as H1 (#)
- Brief description on the line below
- Metadata badges if applicable

### Introduction
- Clear, concise overview of the document's purpose
- Key context or problem statement
- Navigation hints for users

### Main Content
- Use H2 (##) for major sections
- Use H3 (###) for subsections
- Use H4 (####) for detailed subsections
- Maintain consistent indentation (2 spaces)

### Tables
- Use consistent table formatting
- Align columns properly
- Include clear headers
- Add explanatory notes when needed

### Code Blocks
- Use ```text for command examples
- Use ```sh for shell commands
- Use ```powershell for Windows PowerShell commands
- Include proper syntax highlighting

### Lists
- Use consistent bullet point formatting
- Maintain proper indentation
- Include examples where helpful

## Formatting Guidelines

### Text Formatting
- Use **bold** for emphasis
- Use *italic* for emphasis
- Use `code` for inline code
- Use `**code**` for important code elements

### Links
- Use [text](url) format for internal links
- Use proper relative paths
- Validate all links

### Images
- Use ![alt text](path) format
- Include descriptive alt text
- Place images strategically

## Quality Checks

### Before Committing
- Run `git diff --check` for formatting issues
- Validate all internal links
- Check for broken references
- Ensure consistent formatting

### Before Merging
- Review content for completeness
- Validate examples with current code
- Check for outdated information
- Ensure proper cross-referencing

## Example Structure

```markdown
# Document Title

Brief description of the document.

## Section 1

Content for section 1.

### Subsection 1.1

Detailed content for subsection.

```
