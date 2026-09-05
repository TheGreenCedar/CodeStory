"""Benchmark-only, exhaustive reference LateOn execution. No production imports."""
import os
for key, value in {'PYTORCH_ENABLE_MPS_FALLBACK': '0', 'HF_HUB_OFFLINE': '1',
                   'TRANSFORMERS_OFFLINE': '1', 'TOKENIZERS_PARALLELISM': 'false',
                   'OMP_NUM_THREADS': '1', 'OPENBLAS_NUM_THREADS': '1'}.items():
    os.environ[key] = value
import sys, json, time, hashlib, pathlib, signal, importlib.metadata
import numpy as np
import torch
from pylate import models

def digest(data):
    return hashlib.sha256(data).hexdigest()

def publish(path, value):
    with path.open('x') as out:
        json.dump(value, out, separators=(',', ':'), allow_nan=False)

def load_model(input_data):
    assert {d.metadata['Name']: d.version for d in importlib.metadata.distributions()} == input_data['packages'], 'package identity changed'
    root = pathlib.Path(input_data['model_root'])
    for entry in input_data['assets']['entries']:
        assert digest((root / entry['path']).read_bytes()) == entry['sha256']
    assert torch.backends.mps.is_available()
    model = models.ColBERT(str(root), device='mps', local_files_only=True, trust_remote_code=False,
                          model_kwargs={'attn_implementation': 'eager'}, config_kwargs={'reference_compile': False})
    model.eval()
    assert {str(p.device) for p in model.parameters()} == {'mps:0'}
    assert len(model) == 3 and model[1].linear.weight.shape == (512, 256)
    assert model[2].linear.weight.shape == (48, 512)
    assert model.query_length == 256 and model.document_length == 2048
    assert model.query_prefix == '[Q] ' and model.document_prefix == '[D] '
    assert not model.do_query_expansion and model[0].do_lower_case
    # Hostile coordinate preflight: long input must be rejected, never shortened.
    try:
        tokens(model, 'boundary ' * 3000, True)
        raise RuntimeError('oversize query was admitted')
    except AssertionError as error:
        assert str(error) == 'token overflow'
    return model

def tokens(model, text, query):
    model[0].max_seq_length = len(text.encode('utf8')) + 32
    raw = model[0].tokenize([text])['input_ids'][0].tolist()
    expected = raw[:1] + [model.query_prefix_id if query else model.document_prefix_id] + raw[1:]
    assert len(expected) <= (model.query_length if query else model.document_length), 'token overflow'
    actual = model.tokenize([text], is_query=query)['input_ids'][0].tolist()
    assert actual == expected, 'token truncation or normalization drift'
    return {'text_sha256': digest(text.encode()), 'normalized_sha256': digest(text.strip().lower().encode()),
            'token_ids': actual}

def encode(model, texts, query):
    result = model.encode(texts, is_query=query, batch_size=32, convert_to_numpy=False,
                          convert_to_tensor=False, padding=False, pool_factor=1, show_progress_bar=False)
    for vector in result:
        assert vector.device.type == 'mps' and vector.ndim == 2 and vector.shape[1] == 48
        assert len(vector) > 0 and torch.isfinite(vector).all()
        assert torch.max(torch.abs(vector.norm(dim=1) - 1)).item() < 1e-5
    return result

def score_batches(query, batches):
    result = []
    for vectors, mask in batches:
        dots = torch.matmul(query, vectors.transpose(1, 2))
        dots.masked_fill_(~mask[:, None, :], float('-inf'))
        result.extend(dots.max(dim=2).values.sum(dim=1).detach().cpu().tolist())
    return result

def packed(vectors):
    offsets = np.concatenate(([0], np.cumsum([len(v) for v in vectors]))).astype(np.int64)
    return np.concatenate([v.detach().cpu().numpy() for v in vectors]), offsets

def authenticate(fragment, repo):
    root = pathlib.Path(repo['local_root']).resolve()
    path = (root / fragment['path']).resolve()
    assert path.is_relative_to(root)
    data = path.read_bytes()
    assert digest(data) == fragment['content_digest']
    start, end = fragment['byte_range']['start'], fragment['byte_range']['end']
    assert data[start:end].decode('utf8') == fragment['source']
    return end - start

def run(input_path):
    started = time.perf_counter()
    signal.alarm(25 * 60)
    input_bytes = input_path.read_bytes()
    data = json.loads(input_bytes)
    model = load_model(data)
    fragments = data['fragments']
    token_receipts = [tokens(model, f['source'], False) for f in fragments]
    query_preflight = [tokens(model, w['question'], True) for w in data['wordings']]
    docs = encode(model, [f['source'] for f in fragments], False)
    torch.mps.synchronize()
    by_id = {f['fragment_id']: i for i, f in enumerate(fragments)}
    # Fixed padded batches are an exhaustive compute layout, never candidate pruning.
    layouts = {}
    for repo in data['repositories']:
        indices = [by_id[fid] for fid in repo['fragment_ids']]
        layout = []
        for offset in range(0, len(indices), 128):
            selected = [docs[i] for i in indices[offset:offset + 128]]
            lengths = torch.tensor([len(v) for v in selected], device='mps')
            batch = torch.nn.utils.rnn.pad_sequence(selected, batch_first=True)
            mask = torch.arange(batch.shape[1], device='mps')[None, :] < lengths[:, None]
            layout.append((batch, mask))
        layouts[repo['repository_id']] = layout
    torch.mps.synchronize()
    prepare_ms = (time.perf_counter() - started) * 1000
    rows, queries = [], []
    repos = {r['repository_id']: r for r in data['repositories']}
    for ordinal, wording in enumerate(data['wordings']):
        torch.mps.synchronize()
        start = time.perf_counter()
        receipt = tokens(model, wording['question'], True)
        assert receipt == query_preflight[ordinal]
        query = encode(model, [wording['question']], True)[0]
        torch.mps.synchronize()
        encoded = time.perf_counter()
        scores = score_batches(query, layouts[wording['repository_id']])
        torch.mps.synchronize()
        scored = time.perf_counter()
        repo = repos[wording['repository_id']]
        seeds = wording['seed_fragment_ids']
        order = sorted(zip(repo['fragment_ids'], scores), key=lambda x: (-x[1], x[0]))
        excluded = set(seeds)
        legal = seeds + [fid for fid, _ in order if fid not in excluded][:8 * len(seeds)]
        source_bytes = sum(authenticate(fragments[by_id[fid]], repo) for fid in legal)
        end = time.perf_counter()
        queries.append(query)
        rows.append({**{k: wording[k] for k in ('case_id', 'phrasing_id', 'repository_id', 'question_sha256')},
                     'scores': scores, 'seeds': seeds, 'legal': legal, 'source_bytes': source_bytes,
                     'timing': {'whole_ms': (end - start) * 1000, 'query_ms': (encoded - start) * 1000,
                                'scoring_ms': (scored - encoded) * 1000, 'assembly_ms': (end - scored) * 1000,
                                'unaccounted_ms': 0}})
        print(json.dumps({'row': ordinal + 1, 'of': len(data['wordings']), 'ms': rows[-1]['timing']['whole_ms']}), flush=True)
    serialization_start = time.perf_counter()
    dv, do = packed(docs)
    qv, qo = packed(queries)
    vectors_path = input_path.parent / 'vectors.npz'
    with vectors_path.open('xb') as output:
        np.savez(output, documents=dv, document_offsets=do, queries=qv, query_offsets=qo)
    result = {'status': 'outputs_frozen', 'input_sha256': digest(input_bytes),
              'vectors_sha256': digest(vectors_path.read_bytes()), 'rows': rows,
              'document_tokens': token_receipts, 'query_tokens': query_preflight,
              'packages': {d.metadata['Name']: d.version for d in importlib.metadata.distributions()},
              'parameter_device': 'mps:0', 'fallback': False, 'preparation_ms': prepare_ms,
              'vector_serialization_ms': (time.perf_counter() - serialization_start) * 1000,
              'mps_driver_allocated_bytes': torch.mps.driver_allocated_memory(),
              'elapsed_before_result_serialization_ms': (time.perf_counter() - started) * 1000}
    publish(input_path.parent / 'result.json', result)

def verify(input_path):
    data = json.loads(input_path.read_bytes())
    result_bytes = (input_path.parent / 'result.json').read_bytes()
    result = json.loads(result_bytes)
    assert result['input_sha256'] == digest(input_path.read_bytes())
    vector_path = input_path.parent / 'vectors.npz'
    assert result['vectors_sha256'] == digest(vector_path.read_bytes())
    archive = np.load(vector_path, allow_pickle=False)
    assert set(archive.files) == {'documents', 'document_offsets', 'queries', 'query_offsets'}
    def unpack(kind, count):
        vectors, offsets = archive[kind], archive[{'documents': 'document_offsets', 'queries': 'query_offsets'}[kind]]
        assert vectors.dtype == np.float32 and vectors.ndim == 2 and vectors.shape[1] == 48
        assert offsets.dtype == np.int64 and len(offsets) == count + 1
        assert offsets[0] == 0 and offsets[-1] == len(vectors) and np.all(np.diff(offsets) > 0)
        assert np.isfinite(vectors).all() and np.max(np.abs(np.linalg.norm(vectors, axis=1) - 1)) < 1e-5
        return [vectors[a:b] for a, b in zip(offsets[:-1], offsets[1:])]
    docs, queries = unpack('documents', len(data['fragments'])), unpack('queries', len(data['wordings']))
    by_id = {f['fragment_id']: i for i, f in enumerate(data['fragments'])}
    model = load_model(data)
    assert [tokens(model, f['source'], False) for f in data['fragments']] == result['document_tokens']
    assert [tokens(model, w['question'], True) for w in data['wordings']] == result['query_tokens']
    # Independently re-encode a deterministic sample, including first/last and each repo's first document.
    sample = sorted({0, len(docs) - 1, *[by_id[r['fragment_ids'][0]] for r in data['repositories']]})
    fresh = encode(model, [data['fragments'][i]['source'] for i in sample], False)
    for i, vector in zip(sample, fresh):
        assert np.max(np.abs(docs[i] - vector.cpu().numpy())) < 1e-4, 'substituted document vectors'
    fresh_queries = encode(model, [w['question'] for w in data['wordings']], True)
    for old, new in zip(queries, fresh_queries):
        assert np.max(np.abs(old - new.cpu().numpy())) < 1e-4, 'substituted query vectors'
    max_error = 0
    assert len(result['rows']) == len(queries)
    for wording, row, query in zip(data['wordings'], result['rows'], queries):
        repo = next(r for r in data['repositories'] if r['repository_id'] == wording['repository_id'])
        exact = np.array([(query @ docs[by_id[fid]].T).max(axis=1).sum() for fid in repo['fragment_ids']])
        assert len(exact) == len(row['scores'])
        error = float(np.max(np.abs(exact - np.array(row['scores']))))
        max_error = max(max_error, error)
        assert error < 1e-4, 'MaxSim score mismatch'
    publish(input_path.parent / 'vector-validation.json', {'status': 'validated',
            'input_sha256': digest(input_path.read_bytes()), 'result_sha256': digest(result_bytes),
            'vectors_sha256': result['vectors_sha256'], 'maximum_score_error': max_error,
            'sampled_document_indices': sample, 'all_queries_reencoded': True})

if __name__ == '__main__':
    try:
        {'run': run, 'verify': verify}[sys.argv[1]](pathlib.Path(sys.argv[2]).resolve())
    except BaseException as error:
        print(json.dumps({'experiment_status': 'invalid', 'decision': 'not_evaluated', 'error': str(error)}), file=sys.stderr)
        raise
