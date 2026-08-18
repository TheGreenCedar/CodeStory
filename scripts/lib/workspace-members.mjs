// The workspace member crates, read from the one file Cargo itself reads them from.
//
// Several release surfaces need to know "every codestory crate that carries a version": the bump
// writes each one's `[package] version`, the native fingerprint has to normalize exactly those
// lines back out, and `check-codestory-release.py` validates them. Each of those used to keep its
// own hand-written list, so adding a crate to the workspace meant remembering three unrelated
// files. The fingerprint's copy was the dangerous one -- forgetting it does not fail anything, it
// just makes a version-only bump look like a source change and silently voids accelerator evidence
// reuse (#1673). Deriving the list here removes the place the drift could hide.

const WORKSPACE_TABLE = /^\[workspace\]\s*$/mu;
const MEMBERS_ARRAY = /^members\s*=\s*\[([^\]]*)\]/mu;
const QUOTED = /"([^"]+)"/gu;

/// The `[workspace] members` paths declared by a root `Cargo.toml`, in manifest order.
///
/// Takes the manifest source rather than a directory on purpose: the fingerprint has to read the
/// membership recorded at the git ref it is hashing, not whatever the working tree happens to say.
export function parseWorkspaceMembers(manifestSource) {
  const tableStart = manifestSource.search(WORKSPACE_TABLE);
  if (tableStart < 0) throw new Error("root Cargo.toml declares no [workspace] table");
  // Stop at the next table header so `[workspace.dependencies]` and friends cannot contribute.
  const fromTable = manifestSource.slice(tableStart);
  const nextTable = fromTable.slice(1).search(/^\[/mu);
  const table = nextTable < 0 ? fromTable : fromTable.slice(0, nextTable + 1);

  const declared = MEMBERS_ARRAY.exec(table);
  if (!declared) throw new Error("root Cargo.toml [workspace] declares no members array");
  const members = [...declared[1].matchAll(QUOTED)].map(([, member]) => member);
  if (members.length === 0) throw new Error("root Cargo.toml [workspace] members is empty");

  const seen = new Set();
  for (const member of members) {
    if (member.startsWith("/") || member.includes("..") || member.endsWith("/")) {
      throw new Error(`root Cargo.toml [workspace] member ${member} is not a repository path`);
    }
    if (seen.has(member)) {
      throw new Error(`root Cargo.toml [workspace] lists ${member} twice`);
    }
    seen.add(member);
  }
  return members;
}

/// The repository-relative manifest path of every workspace member.
export function workspaceMemberManifests(manifestSource) {
  return parseWorkspaceMembers(manifestSource).map((member) => `${member}/Cargo.toml`);
}

/// The crate name of every workspace member, taken from its directory.
///
/// Every member of this workspace lives at `crates/<crate-name>`; a member that does not is a
/// layout change the version surfaces have to be taught about rather than guess at.
export function workspaceMemberNames(manifestSource) {
  return parseWorkspaceMembers(manifestSource).map((member) => {
    const name = /^crates\/([A-Za-z0-9_-]+)$/u.exec(member)?.[1];
    if (!name) {
      throw new Error(`workspace member ${member} does not live at crates/<crate-name>`);
    }
    return name;
  });
}
