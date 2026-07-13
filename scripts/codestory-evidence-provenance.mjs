export function isTrustedPublishableRepoUrl(url) {
  try {
    const parsed = new URL(String(url ?? ""));
    if (
      parsed.protocol !== "https:"
      || parsed.hostname.toLowerCase() !== "github.com"
      || parsed.username
      || parsed.password
      || parsed.search
      || parsed.hash
    ) {
      return false;
    }
    const parts = parsed.pathname.split("/").filter(Boolean);
    return (
      parts.length === 2
      && /^[A-Za-z0-9_.-]+$/.test(parts[0])
      && /^[A-Za-z0-9_.-]+(?:\.git)?$/.test(parts[1])
    );
  } catch {
    return false;
  }
}

function normalizeTrustedPublishableRepoUrl(url) {
  if (!isTrustedPublishableRepoUrl(url)) {
    return null;
  }
  const parsed = new URL(String(url));
  const [owner, repo] = parsed.pathname.split("/").filter(Boolean);
  return `${owner.toLowerCase()}/${repo.replace(/\.git$/i, "").toLowerCase()}`;
}

export function isImmutableCommitRef(ref) {
  return /^[0-9a-f]{40}$/i.test(String(ref ?? "").trim());
}

function normalizeImmutableCommitRef(ref) {
  const value = String(ref ?? "").trim();
  return isImmutableCommitRef(value) ? value.toLowerCase() : null;
}

export function repoProvenanceBlockers(result) {
  const provenance = result.repo_provenance;
  if (!provenance) {
    return ["missing repo provenance"];
  }
  const reasons = [];
  if (provenance.manifest_overridden_by_builtin) {
    reasons.push("manifest repo was overridden by a built-in checkout");
  }
  const configuredRef = provenance.configured?.ref ?? null;
  const manifestRef = provenance.manifest?.ref ?? null;
  const configuredCommit = normalizeImmutableCommitRef(configuredRef);
  const manifestCommit = manifestRef ? normalizeImmutableCommitRef(manifestRef) : null;
  const gitHead = normalizeImmutableCommitRef(provenance.git_head);
  if (!configuredCommit) {
    reasons.push("repo ref is not pinned to a full immutable commit SHA");
  }
  if (manifestRef && configuredRef && manifestCommit !== configuredCommit) {
    reasons.push(`manifest ref ${manifestRef} does not match configured ref ${configuredRef}`);
  }
  if (!gitHead) {
    reasons.push("missing git head");
  } else if (configuredCommit && gitHead !== configuredCommit) {
    reasons.push(`git head ${provenance.git_head} does not match configured ref ${configuredRef}`);
  }
  const configuredUrl = provenance.configured?.url ?? null;
  const manifestUrl = provenance.manifest?.url ?? null;
  const gitOrigin = provenance.git_origin ?? null;
  const configuredRepo = normalizeTrustedPublishableRepoUrl(configuredUrl);
  const manifestRepo = manifestUrl ? normalizeTrustedPublishableRepoUrl(manifestUrl) : null;
  const originRepo = gitOrigin ? normalizeTrustedPublishableRepoUrl(gitOrigin) : null;
  if (!configuredRepo) {
    reasons.push("configured repo URL is not a trusted GitHub HTTPS repo URL");
  }
  if (!manifestUrl) {
    reasons.push("missing manifest repo URL");
  } else if (!manifestRepo) {
    reasons.push("manifest repo URL is not a trusted GitHub HTTPS repo URL");
  }
  if (configuredRepo && manifestUrl && manifestRepo && manifestRepo !== configuredRepo) {
    reasons.push(`manifest repo URL ${manifestUrl} does not match configured URL ${configuredUrl}`);
  }
  if (!originRepo) {
    reasons.push("git origin is missing or is not a trusted GitHub HTTPS repo URL");
  } else if (configuredRepo && originRepo !== configuredRepo) {
    reasons.push(`git origin ${gitOrigin} does not match configured URL ${configuredUrl}`);
  }
  if (provenance.git_dirty !== false) {
    reasons.push(provenance.git_dirty ? "repo checkout is dirty" : "repo cleanliness is unknown");
  }
  return reasons;
}

export function cacheProvenanceBlockers(result) {
  const provenance = result.codestory_cache_provenance;
  if (!provenance) {
    return ["missing CodeStory cache provenance"];
  }
  const reasons = [];
  if (provenance.doctor_status !== "pass") {
    reasons.push("CodeStory doctor provenance failed");
  }
  if (!provenance.storage_path) {
    reasons.push("missing CodeStory cache path");
  }
  if (!provenance.cache_policy) {
    reasons.push("missing CodeStory cache policy");
  }
  if (provenance.cache_policy === "unprepared-cache-blocked") {
    reasons.push("CodeStory sidecar cache was not prepared");
  }
  if (provenance.retrieval_mode !== "full") {
    reasons.push(`CodeStory retrieval mode=${provenance.retrieval_mode ?? "unknown"}; expected full`);
  }
  if (!provenance.sidecar_generation) {
    reasons.push("missing CodeStory sidecar generation");
  }
  if (provenance.manifest_embedding_backend !== "llamacpp:bge-base-en-v1.5") {
    reasons.push(
      `CodeStory sidecar embedding backend=${provenance.manifest_embedding_backend ?? "unknown"}; expected llamacpp:bge-base-en-v1.5`,
    );
  }
  if (provenance.semantic_backend == null) {
    reasons.push("missing CodeStory semantic backend");
  }
  if (provenance.local_only !== true) {
    reasons.push(`CodeStory local-only guarantee is not proven (${provenance.locality_kind ?? "unknown"})`);
  }
  if (provenance.indexed !== true) {
    reasons.push("CodeStory cache is not indexed");
  }
  if (provenance.freshness_status !== "fresh") {
    reasons.push(`CodeStory cache freshness=${provenance.freshness_status ?? "unknown"}`);
  }
  if (provenance.semantic_ready !== true) {
    reasons.push("CodeStory semantic docs are not ready");
  }
  if (provenance.indexing_in_timed_run == null) {
    reasons.push("missing timed-run indexing provenance");
  }
  return reasons;
}
