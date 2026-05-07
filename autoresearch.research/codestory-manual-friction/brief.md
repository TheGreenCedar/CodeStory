# Research Brief: Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

## Request
Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

## Decision To Support
- Identify source-backed changes worth testing through an autoresearch loop.

## Success Criteria
- `quality_gap=0` from the full three-repo harness.
- `doctor`, `ask --investigate`, `search`, `symbol`, `trail`, and `snippet` complete the skill-approved explanation path for `../Sourcetrail`, `../rootandruntime`, and `.`.
- Two fresh consecutive full rounds add no new P0/P1/P2 friction.
- Each implemented or rejected gap has evidence and ASI in the autoresearch ledger.

## Constraints
- Implementation remains in CodeStory unless the loop exposes a plugin-specific blocker.
- Short targeted tests can guide implementation; only the full three-repo harness can close the quality-gap loop.

## Known Unknowns
- Whether semantic setup is already installed on every future operator machine.
- Whether the current bad-term drift checks need expansion after the first clean benchmark round.
