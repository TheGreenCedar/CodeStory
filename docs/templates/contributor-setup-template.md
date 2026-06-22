# Contributor Setup Template

## Overview

This template is for documenting contributor setup and verification procedures.

## Required Sections

### Setup Overview
- Brief explanation of why CodeStory exists
- Explanation of the grounding loop for contributors
- Example prompt for working in the repository

### Verification Lane Picker
- Clear explanation of verification lanes
- Table showing which lane to use for different changes
- Clear escalation criteria

### Crate Ownership
- Explanation of crate ownership
- Mermaid flowchart for decision making
- Clear mapping of crates to behaviors

### Basic Cargo Lane
- Standard commands for routine verification
- Explanation of why commands should be run serially
- Guidance on when to use this lane

### Local CLI Loop
- Step-by-step CLI verification process
- Explanation of why the built binary should be used
- Clear examples of verification commands

### Hybrid Retrieval Setup
- Explanation of when to use this lane
- Required setup commands
- Explanation of configuration options

### Recommended Reading Order
- Ordered list of documentation to build mental models
- Clear progression from high-level to detailed

### Rustdoc Baseline
- Commands for running rustdoc baseline
- Explanation of why rustdoc is important
- Guidance on public API documentation

## Example Structure

Use four-space indentation for nested code blocks inside the example skeleton
below. Do not nest triple-backtick fences.

    # Contributor Setup

    CodeStory exists because agents otherwise rediscover the same repository on
    every question. When you change CodeStory itself, use the same grounding loop
    you ship to users: check readiness, ground the checkout, then trace the owning
    crate before editing.

    Example prompt (CodeStory repo):

        @CodeStory Where is RefreshMode defined, which codestory-cli commands accept --refresh, and what is the call path from index into codestory-store?

    ## Choose The Verification Lane First

    Before running Cargo or setting up sidecars, answer two questions:

    1. Which crate owns the behavior?
    2. What is the smallest proof that covers the change?

    | Change | Start here | Escalate when |
    | --- | --- | --- |
    | Docs only | Read changed pages back, then run git diff --check | Doc depends on new code behavior or release evidence |
