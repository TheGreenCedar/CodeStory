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

function isSha256(value) {
  return /^[0-9a-f]{64}$/i.test(String(value ?? "").trim());
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
  const declaredProjectManifest = provenance.manifest?.codestory_project_manifest ?? null;
  const installedProjectManifest = provenance.installed_codestory_project_manifest ?? null;
  if (declaredProjectManifest) {
    if (!isSha256(declaredProjectManifest.sha256) || !declaredProjectManifest.path) {
      reasons.push("manifest CodeStory project manifest declaration is invalid");
    }
    if (!installedProjectManifest) {
      reasons.push("missing installed CodeStory project manifest provenance");
    } else {
      if (installedProjectManifest.declared_sha256 !== declaredProjectManifest.sha256) {
        reasons.push("installed CodeStory project manifest does not match declared manifest hash");
      }
      if (installedProjectManifest.installed_sha256 !== declaredProjectManifest.sha256) {
        reasons.push("installed CodeStory project manifest bytes do not match declared hash");
      }
      if (installedProjectManifest.ignored !== true) {
        reasons.push("installed CodeStory project manifest is not ignored by the checkout");
      }
      if (installedProjectManifest.installed_path !== "codestory_project.json") {
        reasons.push("installed CodeStory project manifest is not rooted at codestory_project.json");
      }
    }
  } else if (installedProjectManifest) {
    reasons.push("unexpected installed CodeStory project manifest provenance");
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
    reasons.push("CodeStory retrieval cache was not prepared");
  }
  if (provenance.retrieval_mode !== "full") {
    reasons.push(`CodeStory retrieval mode=${provenance.retrieval_mode ?? "unknown"}; expected full`);
  }
  if (!provenance.semantic_generation) {
    reasons.push("missing CodeStory semantic generation");
  }
  if (!String(provenance.manifest_embedding_backend ?? "").startsWith("per-user-server:coderank-embed:q8_0:sha256-")) {
    reasons.push(
      `CodeStory embedding runtime=${provenance.manifest_embedding_backend ?? "unknown"}; expected the pinned per-user CodeRankEmbed server runtime`,
    );
  }
  const packetExecutionReasons = packetEmbeddingExecutionProofBlockers(provenance);
  const packetExecutionProven = packetExecutionReasons.length === 0;
  if (!provenance.embedding_engine_instance_id && !packetExecutionProven) {
    reasons.push("missing CodeStory embedding engine identity");
  }
  if (provenance.embedding_policy !== "accelerated") {
    reasons.push(`CodeStory embedding policy=${provenance.embedding_policy ?? "unknown"}; expected accelerated`);
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
  if (provenance.semantic_ready !== true && !packetExecutionProven) {
    reasons.push("CodeStory semantic docs are not ready");
  }
  if (provenance.packet_embedding_execution && !packetExecutionProven) {
    reasons.push(...packetExecutionReasons);
  }
  if (provenance.indexing_in_timed_run == null) {
    reasons.push("missing timed-run indexing provenance");
  }
  return reasons;
}

export function packetEmbeddingExecutionProofBlockers(provenance) {
  const proof = provenance?.packet_embedding_execution;
  if (!proof) {
    return ["missing cold packet embedding execution proof"];
  }
  if (proof.source === "packet.v3_public_projection") {
    return packetV3PublicProjectionProofBlockers(provenance, proof);
  }
  const reasons = [];
  if (proof.source !== "packet.answer.retrieval_trace") {
    reasons.push(`cold packet embedding execution source=${proof.source ?? "unknown"}; expected packet.answer.retrieval_trace`);
  }
  if (!["cold_cli_packet", "agent_harness_prelude"].includes(proof.transport_mode)) {
    reasons.push(`cold packet embedding execution transport=${proof.transport_mode ?? "unknown"}; expected cold_cli_packet or agent_harness_prelude`);
  }
  if (proof.retrieval_contract !== "in_process_v1") {
    reasons.push(`cold packet retrieval contract=${proof.retrieval_contract ?? "unknown"}; expected in_process_v1`);
  }
  if (proof.embedding_engine !== "process_shared") {
    reasons.push(`cold packet embedding engine=${proof.embedding_engine ?? "unknown"}; expected process_shared`);
  }
  if (proof.embedding_policy !== "accelerated") {
    reasons.push(`cold packet embedding policy=${proof.embedding_policy ?? "unknown"}; expected accelerated`);
  }
  if (proof.retrieval_mode !== "full") {
    reasons.push(`cold packet retrieval mode=${proof.retrieval_mode ?? "unknown"}; expected full`);
  }
  if (!Number.isInteger(proof.diagnostic_count) || proof.diagnostic_count <= 0) {
    reasons.push("cold packet embedding execution has no sidecar diagnostics");
  }
  if (proof.full_diagnostic_count !== proof.diagnostic_count) {
    reasons.push("cold packet embedding execution contains a non-full sidecar diagnostic");
  }
  if (!Number.isInteger(proof.semantic_stage_count) || proof.semantic_stage_count <= 0) {
    reasons.push("cold packet embedding execution has no semantic stage");
  }
  if (proof.completed_semantic_stage_count !== proof.semantic_stage_count) {
    reasons.push("cold packet embedding execution contains an incomplete semantic stage");
  }
  if (proof.invalid_semantic_stage_count !== 0) {
    reasons.push("cold packet embedding execution contains a degraded, stubbed, or cancelled semantic stage");
  }
  if (proof.shadow_degraded_reason != null) {
    reasons.push("cold packet retrieval shadow is degraded");
  }
  if (proof.shadow_error != null) {
    reasons.push("cold packet retrieval shadow contains an error");
  }
  if (proof.shadow_cancel_reason != null) {
    reasons.push("cold packet retrieval shadow was cancelled");
  }
  if (proof.semantic_fallback_count !== 0) {
    reasons.push(`cold packet semantic fallback count=${proof.semantic_fallback_count ?? "unknown"}; expected 0`);
  }
  if (!proof.semantic_generation || !proof.prepared_semantic_generation) {
    reasons.push("cold packet embedding execution is missing semantic generation identity");
  } else if (proof.semantic_generation !== proof.prepared_semantic_generation) {
    reasons.push("cold packet semantic generation does not match the prepared generation");
  }
  if (
    provenance?.semantic_generation
    && proof.prepared_semantic_generation
    && provenance.semantic_generation !== proof.prepared_semantic_generation
  ) {
    reasons.push("cold packet prepared semantic generation does not match cache provenance");
  }
  if (
    provenance?.transport_mode
    && proof.transport_mode
    && provenance.transport_mode !== proof.transport_mode
  ) {
    reasons.push("cold packet transport does not match cache provenance");
  }
  if (
    provenance?.embedding_policy
    && proof.embedding_policy
    && provenance.embedding_policy !== proof.embedding_policy
  ) {
    reasons.push("cold packet embedding policy does not match cache provenance");
  }
  return reasons;
}

function packetV3PublicProjectionProofBlockers(provenance, proof) {
  const reasons = [];
  if (proof.schema_version !== 3) {
    reasons.push(`cold packet public projection schema=${proof.schema_version ?? "unknown"}; expected 3`);
  }
  if (!["cold_cli_packet", "agent_harness_prelude"].includes(proof.transport_mode)) {
    reasons.push(`cold packet embedding execution transport=${proof.transport_mode ?? "unknown"}; expected cold_cli_packet or agent_harness_prelude`);
  }
  if (proof.retrieval_contract !== "in_process_v1") {
    reasons.push(`cold packet retrieval contract=${proof.retrieval_contract ?? "unknown"}; expected in_process_v1`);
  }
  if (proof.embedding_engine !== "process_shared") {
    reasons.push(`cold packet embedding engine=${proof.embedding_engine ?? "unknown"}; expected process_shared`);
  }
  if (proof.embedding_policy !== "accelerated") {
    reasons.push(`cold packet embedding policy=${proof.embedding_policy ?? "unknown"}; expected accelerated`);
  }
  if (proof.retrieval_mode !== "full") {
    reasons.push(`cold packet retrieval mode=${proof.retrieval_mode ?? "unknown"}; expected full`);
  }
  if (!["complete", "budget_exceeded"].includes(proof.packet_kind)) {
    reasons.push(`cold packet public projection kind=${proof.packet_kind ?? "unknown"} is invalid`);
  }
  if (!["available", "continuation_available", "no_useful_evidence", "unavailable"].includes(proof.evidence_status)) {
    reasons.push(`cold packet evidence status=${proof.evidence_status ?? "unknown"} is invalid`);
  }
  if (!Number.isInteger(proof.evidence_count) || proof.evidence_count < 0) {
    reasons.push("cold packet public projection has invalid evidence accounting");
  }
  if (!Number.isInteger(proof.gap_count) || proof.gap_count < 0) {
    reasons.push("cold packet public projection has invalid gap accounting");
  }
  if (
    proof.packet_kind === "budget_exceeded" &&
    (proof.evidence_count !== 0 || proof.gap_count <= 0 || proof.evidence_status !== "unavailable")
  ) {
    reasons.push("cold packet budget fallback contains invalid evidence or gap accounting");
  }
  if (!proof.core_generation || !proof.core_run_id) {
    reasons.push("cold packet public projection is missing core publication identity");
  }
  if (
    proof.retrieval_core_generation !== proof.core_generation ||
    proof.retrieval_core_run_id !== proof.core_run_id
  ) {
    reasons.push("cold packet retrieval publication does not match its core publication");
  }
  if (
    !proof.retrieval_generation ||
    proof.retrieval_state_generation !== proof.retrieval_generation
  ) {
    reasons.push("cold packet retrieval state generation does not match its publication");
  }
  if (!proof.semantic_generation || !proof.prepared_semantic_generation) {
    reasons.push("cold packet public projection is missing semantic generation identity");
  } else if (proof.semantic_generation !== proof.prepared_semantic_generation) {
    reasons.push("cold packet semantic generation does not match the prepared generation");
  }
  if (
    provenance?.semantic_generation &&
    proof.prepared_semantic_generation &&
    provenance.semantic_generation !== proof.prepared_semantic_generation
  ) {
    reasons.push("cold packet prepared semantic generation does not match cache provenance");
  }
  if (
    proof.diagnostics_availability !== "available" ||
    !String(proof.diagnostics_artifact_id ?? "").trim() ||
    !isSha256(proof.diagnostics_sha256) ||
    !Number.isInteger(proof.diagnostics_byte_length) ||
    proof.diagnostics_byte_length < 0
  ) {
    reasons.push("cold packet public projection has no valid diagnostics reference");
  }
  if (
    provenance?.transport_mode &&
    proof.transport_mode &&
    provenance.transport_mode !== proof.transport_mode
  ) {
    reasons.push("cold packet transport does not match cache provenance");
  }
  if (
    provenance?.embedding_policy &&
    proof.embedding_policy &&
    provenance.embedding_policy !== proof.embedding_policy
  ) {
    reasons.push("cold packet embedding policy does not match cache provenance");
  }
  return reasons;
}
